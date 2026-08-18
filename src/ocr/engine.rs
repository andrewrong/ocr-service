use std::sync::Arc;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    models::{Engine, ModelStatus},
};

const OCR_PROMPT: &str = "Transcribe this document image faithfully into Markdown. Preserve headings, paragraphs, lists, tables, formulas, reading order, and line breaks where meaningful. Do not summarize, explain, or wrap the result in a code fence. Return only the transcription.";

#[derive(Clone)]
pub struct OcrEngine {
    client: Client,
    config: Arc<Config>,
}

#[derive(Debug)]
pub struct OcrOutput {
    pub markdown: String,
    pub engine: Engine,
}

impl OcrEngine {
    pub fn new(config: Arc<Config>) -> Result<Self> {
        let client = Client::builder()
            .timeout(config.request_timeout)
            .build()
            .context("failed to build Ollama HTTP client")?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn resolved_engine(&self, engine: Engine) -> Engine {
        match engine {
            Engine::Auto => Engine::Paddle,
            explicit => explicit,
        }
    }

    pub fn model_name(&self, engine: Engine) -> &str {
        match self.resolved_engine(engine) {
            Engine::Paddle => &self.config.paddle_model,
            Engine::Glm => &self.config.glm_model,
            Engine::Qwen => &self.config.qwen_model,
            Engine::Auto => unreachable!("auto is resolved above"),
        }
    }

    pub async fn ocr_image(&self, image: &[u8], engine: Engine) -> Result<OcrOutput> {
        anyhow::ensure!(!image.is_empty(), "image is empty");

        let engine = self.resolved_engine(engine);
        let model = self.model_name(engine);
        let request = ChatRequest {
            model,
            messages: vec![ChatMessage {
                role: "user",
                content: OCR_PROMPT,
                images: vec![STANDARD.encode(image)],
            }],
            stream: false,
            options: ChatOptions { temperature: 0 },
        };

        let response = self
            .client
            .post(format!("{}/api/chat", self.config.ollama_url))
            .json(&request)
            .send()
            .await
            .with_context(|| {
                format!("failed to connect to Ollama at {}", self.config.ollama_url)
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama returned {status} for model {model}: {body}");
        }

        let response: ChatResponse = response
            .json()
            .await
            .context("Ollama returned an invalid chat response")?;
        let markdown = response.message.content.trim().to_owned();
        anyhow::ensure!(
            !markdown.is_empty(),
            "Ollama returned an empty transcription"
        );

        Ok(OcrOutput { markdown, engine })
    }

    pub async fn health(&self) -> Result<Vec<ModelStatus>> {
        let response = self
            .client
            .get(format!("{}/api/tags", self.config.ollama_url))
            .send()
            .await
            .with_context(|| {
                format!("failed to connect to Ollama at {}", self.config.ollama_url)
            })?;

        if response.status() != StatusCode::OK {
            anyhow::bail!("Ollama health check returned {}", response.status());
        }

        let tags: TagsResponse = response
            .json()
            .await
            .context("Ollama returned an invalid tags response")?;
        let installed: Vec<&str> = tags
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect();

        Ok([Engine::Paddle, Engine::Glm, Engine::Qwen]
            .into_iter()
            .map(|engine| {
                let name = self.model_name(engine).to_owned();
                ModelStatus {
                    engine,
                    available: installed
                        .iter()
                        .any(|candidate| model_names_match(candidate, &name)),
                    name,
                }
            })
            .collect())
    }
}

fn model_names_match(installed: &str, configured: &str) -> bool {
    installed == configured
        || installed.strip_suffix(":latest") == Some(configured)
        || configured.strip_suffix(":latest") == Some(installed)
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    options: ChatOptions,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
    images: Vec<String>,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: u8,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::model_names_match;

    #[test]
    fn latest_tag_is_optional_when_matching_names() {
        assert!(model_names_match("glm-ocr:latest", "glm-ocr"));
        assert!(model_names_match("glm-ocr", "glm-ocr:latest"));
        assert!(!model_names_match("glm-ocr:v2", "glm-ocr"));
    }
}
