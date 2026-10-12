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
            stream: false,
            temperature: 0,
            max_tokens: self.max_tokens,
        };

        let response = self
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

        let response: ChatCompletionResponse = response
            .json()
            .await
            .with_context(|| format!("{} returned an invalid chat response", self.label))?;
        let choice = response.choices.into_iter().next();
        let truncated = choice
            .as_ref()
            .is_some_and(|choice| choice.finish_reason.as_deref() == Some("length"));
        let mut markdown = choice
            .map(|choice| choice.message.content.trim().to_owned())
            .unwrap_or_default();
        let mut repetition_removed = None;
        if truncated {
            let (cut, removed) = cut_repetition(&markdown)
                .with_context(|| format!("{} returned a truncated transcription", self.label))?;
            markdown = cut;
            repetition_removed = Some(removed);
        }
        anyhow::ensure!(
            !markdown.is_empty(),
            "{} returned an empty transcription",
            self.label
        );
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
    for k in 1..=n / 3 {
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
