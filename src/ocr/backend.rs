use anyhow::{Context, Result};
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Clone)]
pub(super) struct OpenAiCompatibleBackend {
    client: Client,
    base_url: String,
    api_token: Option<String>,
    label: String,
    max_tokens: u32,
}

/// While streaming, the text is checked for a loop after this many new characters.
const CHECK_EVERY_CHARS: usize = 500;
/// A loop found during the stream is a unit written at least this many times in a row.
const MIN_LOOP_COPIES: usize = 3;
/// A loop found during the stream repeats at least this many characters in all.
const MIN_LOOP_CHARS: usize = 200;
/// The longest unit of a loop found during the stream, in characters.
const MAX_LOOP_UNIT: usize = 500;

/// A local model's text, and how many characters a cut repetition removed from it.
pub(super) struct Transcription {
    pub(super) markdown: String,
    pub(super) repetition_removed: Option<usize>,
}

impl OpenAiCompatibleBackend {
    pub(super) fn new(config: &Config) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("failed to build inference HTTP client")?;
        Ok(Self {
            client,
            base_url: config.inference_url.clone(),
            api_token: config.inference_api_token.clone(),
            label: config.inference_backend.to_string(),
            max_tokens: config.local_max_output_tokens,
        })
    }

    pub(super) async fn transcribe(
        &self,
        model: &str,
        prompt: &str,
        image_data_url: &str,
    ) -> Result<Transcription> {
        let request = ChatCompletionRequest {
            model,
            messages: vec![ChatMessage {
                role: "user",
                content: vec![
                    ContentPart::Text { text: prompt },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: image_data_url,
                        },
                    },
                ],
            }],
            stream: true,
            temperature: 0,
            max_tokens: self.max_tokens,
        };

        let mut response = self
            .with_auth(
                self.client
                    .post(format!("{}/v1/chat/completions", self.base_url)),
            )
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to connect to {} at {}", self.label, self.base_url))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("{} returned {status} for model {model}: {body}", self.label);
        }

        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("application/json"));
        let mut reader = StreamReader::default();
        if is_json {
            // A server that ignores `stream` answers with one JSON document.
            let body: ChatCompletionResponse = response
                .json()
                .await
                .with_context(|| format!("{} returned an invalid chat response", self.label))?;
            if let Some(choice) = body.choices.into_iter().next() {
                reader.text = choice.message.content;
                reader.finish_reason = choice.finish_reason;
            }
        } else {
            let mut checked = 0;
            let mut pending = Vec::new();
            'stream: while let Some(chunk) = response
                .chunk()
                .await
                .with_context(|| format!("{} stream failed", self.label))?
            {
                pending.extend_from_slice(&chunk);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<u8> = pending.drain(..=end).collect();
                    if reader.line(&line) {
                        break 'stream;
                    }
                }
                if reader.chars - checked >= CHECK_EVERY_CHARS {
                    checked = reader.chars;
                    if let Some(kept) = find_loop(&reader.text) {
                        // Dropping `response` closes the connection to the model.
                        return self.finish(&reader.text, Some(reader.text[..kept].to_owned()));
                    }
                }
            }
            reader.line(&pending);
        }

        let truncated = reader.finish_reason.as_deref() == Some("length");
        if truncated {
            let text = reader.text.trim();
            let cut = cut_repetition(text)
                .map(|(cut, _)| cut)
                .or_else(|| find_loop(text).map(|kept| text[..kept].to_owned()))
                .with_context(|| format!("{} returned a truncated transcription", self.label))?;
            return self.finish(text, Some(cut));
        }
        self.finish(&reader.text, None)
    }

    /// Builds the result from `text`, or from `cut`, the text without its looping tail.
    fn finish(&self, text: &str, cut: Option<String>) -> Result<Transcription> {
        let removed = cut.is_some();
        let markdown = cut.as_deref().unwrap_or(text).trim().to_owned();
        let text = text.trim();
        anyhow::ensure!(
            !markdown.is_empty(),
            "{} returned an empty transcription",
            self.label
        );
        let repetition_removed = removed.then(|| text.chars().count() - markdown.chars().count());
        Ok(Transcription {
            markdown,
            repetition_removed,
        })
    }

    pub(super) async fn model_names(&self) -> Result<Vec<String>> {
        let response = self
            .with_auth(self.client.get(format!("{}/v1/models", self.base_url)))
            .send()
            .await
            .with_context(|| format!("failed to connect to {} at {}", self.label, self.base_url))?;

        if response.status() != StatusCode::OK {
            anyhow::bail!("{} health check returned {}", self.label, response.status());
        }

        let models: ModelsResponse = response
            .json()
            .await
            .with_context(|| format!("{} returned an invalid models response", self.label))?;
        Ok(models.data.into_iter().map(|model| model.id).collect())
    }

    fn with_auth(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.api_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
}

/// Cuts a looping tail from `text`: the end must be one block of lines written at least three
/// times in a row, the last copy possibly incomplete. Returns the text up to and including the
/// first copy and the number of characters removed, or `None` when the end is no repetition.
fn cut_repetition(text: &str) -> Option<(String, usize)> {
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();
    let mut best: Option<(usize, usize)> = None; // (lines kept, block length)
    for k in 1..=n.saturating_sub(1) / 2 {
        // The last line may be a prefix of the line one block earlier.
        let matches = |i: usize| {
            if i + k == n - 1 {
                lines[i].starts_with(lines[n - 1])
            } else {
                lines[i] == lines[i + k]
            }
        };
        let mut start = n - k;
        while start > 0 && matches(start - 1) {
            start -= 1;
        }
        // `start` is now the first line of the periodic tail; it needs two full blocks and more.
        if n - start > 2 * k {
            let keep = start + k;
            if best.is_none_or(|(kept, _)| keep < kept) {
                best = Some((keep, k));
            }
        }
    }
    let (keep, _) = best?;
    let cut = lines[..keep].join("\n");
    let removed = text.chars().count() - cut.chars().count();
    Some((cut, removed))
}

/// The text and the finish reason read so far from a server-sent event stream.
#[derive(Default)]
struct StreamReader {
    text: String,
    chars: usize,
    finish_reason: Option<String>,
}

impl StreamReader {
    /// Reads one line of the stream; returns true at `data: [DONE]`.
    fn line(&mut self, line: &[u8]) -> bool {
        let line = String::from_utf8_lossy(line);
        let Some(data) = line.trim().strip_prefix("data:") else {
            return false;
        };
        let data = data.trim();
        if data == "[DONE]" {
            return true;
        }
        if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data)
            && let Some(choice) = chunk.choices.into_iter().next()
        {
            if let Some(content) = choice.delta.content {
                self.chars += content.chars().count();
                self.text.push_str(&content);
            }
            if choice.finish_reason.is_some() {
                self.finish_reason = choice.finish_reason;
            }
        }
        false
    }
}

/// Finds a loop at the end of `text`: a unit of up to `MAX_LOOP_UNIT` characters (lines or not)
/// written at least `MIN_LOOP_COPIES` times in a row, `MIN_LOOP_CHARS` characters in all, the
/// last copy possibly incomplete. Returns the byte length of the text to keep: up to the first copy
/// of a block of lines, or up to the start of a run inside one line.
fn find_loop(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut best: Option<usize> = None; // chars kept
    for unit in 1..=MAX_LOOP_UNIT.min(n / MIN_LOOP_COPIES) {
        let mut start = n - unit;
        while start > 0 && chars[start - 1].1 == chars[start - 1 + unit].1 {
            start -= 1;
        }
        let repeated = n - start;
        // A block of lines keeps its first copy; a run inside a line is all loop.
        let whole_lines = chars[start..start + unit].iter().any(|(_, c)| *c == '\n');
        let keep = if whole_lines { start + unit } else { start };
        if repeated >= MIN_LOOP_COPIES * unit
            && repeated >= MIN_LOOP_CHARS
            && best.is_none_or(|kept| keep < kept)
        {
            best = Some(keep);
        }
    }
    let kept = best?;
    Some(chars.get(kept).map_or(text.len(), |(offset, _)| *offset))
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    temperature: u8,
    max_tokens: u32,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: Vec<ContentPart<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart<'a> {
    Text { text: &'a str },
    ImageUrl { image_url: ImageUrl<'a> },
}

#[derive(Serialize)]
struct ImageUrl<'a> {
    url: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ResponseMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

#[cfg(test)]
mod tests {
    use reqwest::header::AUTHORIZATION;

    use super::OpenAiCompatibleBackend;
    use crate::config::{Config, InferenceBackend};

    #[test]
    fn optional_api_token_adds_bearer_authorization() {
        let backend = OpenAiCompatibleBackend::new(&Config {
            inference_backend: InferenceBackend::LmStudio,
            inference_url: "http://ocr.test".to_owned(),
            inference_api_token: Some("test-token".to_owned()),
            ..Config::default()
        })
        .expect("backend should initialize");
        let request = backend
            .with_auth(backend.client.get("http://ocr.test/v1/models"))
            .build()
            .expect("request should build");

        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .expect("authorization header should exist"),
            "Bearer test-token"
        );
    }
}
