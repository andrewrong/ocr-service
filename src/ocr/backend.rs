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
        })
    }

    pub(super) async fn transcribe(
        &self,
        model: &str,
        prompt: &str,
        image_data_url: &str,
    ) -> Result<String> {
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
        let markdown = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.trim().to_owned())
            .unwrap_or_default();
        anyhow::ensure!(
            !markdown.is_empty(),
            "{} returned an empty transcription",
            self.label
        );
        Ok(markdown)
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

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    temperature: u8,
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
