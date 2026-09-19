use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::{Value, json};

use crate::config::Config;

pub(super) const MODEL: &str = "jina-ocr-v1";
const PROMPT: &str = "Transcribe the provided document image into a clean Markdown format, preserving the natural reading order.";

#[derive(Clone)]
pub(super) struct JinaBackend {
    client: Client,
    base_url: String,
    api_key: Option<String>,
}

/// Missing cloud credentials are a caller configuration error, not model fallback.
#[derive(Debug)]
pub struct JinaNotConfigured;

impl std::fmt::Display for JinaNotConfigured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Jina OCR requires OCR_JINA_API_KEY in the private deployment environment")
    }
}

impl std::error::Error for JinaNotConfigured {}

impl JinaBackend {
    pub(super) fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            client: Client::builder().timeout(config.request_timeout).build()?,
            base_url: config.jina_base_url.clone(),
            api_key: config.jina_api_key.clone(),
        })
    }

    pub(super) fn configured(&self) -> bool {
        self.api_key
            .as_ref()
            .is_some_and(|key| !key.trim().is_empty())
    }

    pub(super) async fn reachable(&self) -> bool {
        if !self.configured() {
            return false;
        }
        let result = self
            .client
            .get(format!("{}/v1/models/{MODEL}", self.base_url))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await;
        let Ok(response) = result else {
            return false;
        };
        if !response.status().is_success() {
            return false;
        }
        let Ok(body) = response.json::<Value>().await else {
            return false;
        };
        matches!(
            body["id"].as_str(),
            Some("jina-ocr-v1" | "jina-ai/jina-ocr-v1")
        )
    }

    pub(super) async fn transcribe(&self, image_data_url: &str) -> Result<String> {
        let key = self.api_key.as_ref().ok_or(JinaNotConfigured)?;
        let request = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(key)
            .json(
                &json!({"model": MODEL, "stream": false, "max_completion_tokens": 8192,
                "messages": [{"role": "user", "content": [
                    {"type": "text", "text": PROMPT},
                    {"type": "image_url", "image_url": {"url": image_data_url}}
                ]}]}),
            );
        let mut response = request
            .try_clone()
            .context("failed to build Jina request")?
            .send()
            .await
            .context("Jina OCR transport failed")?;
        let status = response.status();
        if matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504) {
            let default = if status.as_u16() == 503 { 30 } else { 2 };
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(default)
                .min(60);
            tracing::info!(status = %status, delay_seconds = delay, "retrying Jina OCR once");
            drop(response);
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
            response = request
                .send()
                .await
                .context("Jina OCR retry transport failed")?;
        }
        // Never propagate upstream bodies: they may echo credentials or document data.
        anyhow::ensure!(
            response.status().is_success(),
            "Jina OCR returned HTTP {}",
            response.status()
        );
        let body: Value = response
            .json()
            .await
            .context("Jina OCR returned invalid JSON")?;
        anyhow::ensure!(
            body["choices"][0]["finish_reason"] != "length",
            "Jina OCR returned a truncated transcription"
        );
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim();
        anyhow::ensure!(
            !content.is_empty(),
            "Jina OCR returned an empty transcription"
        );
        Ok(content.to_owned())
    }
}
