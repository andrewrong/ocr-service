use std::sync::Arc;

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::{
    config::Config,
    models::{Engine, ModelStatus},
};

const OCR_PROMPT: &str = "Transcribe this document image faithfully into Markdown. Preserve headings, paragraphs, lists, tables, formulas, reading order, and line breaks where meaningful. Do not summarize, explain, or wrap the result in a code fence. Return only the transcription.";

#[derive(Clone)]
pub struct OcrEngine {
    client: Client,
    config: Arc<Config>,
    request_slots: Arc<Semaphore>,
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
        let request_slots = Arc::new(Semaphore::new(config.max_concurrent_model_requests));
        Ok(Self {
            client,
            config,
            request_slots,
        })
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

        let encoded_image = STANDARD.encode(image);
        let fallback_order = fallback_order(engine);
        let mut timed_out = Vec::new();

        for (index, candidate) in fallback_order.into_iter().enumerate() {
            let timeout = self.config.timeout_for(candidate);
            let permit = self
                .request_slots
                .acquire()
                .await
                .context("OCR model request limiter closed")?;
            let attempt =
                tokio::time::timeout(timeout, self.ocr_with_engine(&encoded_image, candidate))
                    .await;
            drop(permit);

            match attempt {
                Ok(result) => return result,
                Err(_) => {
                    timed_out.push(format!("{candidate} ({}s)", timeout.as_secs()));
                    let next_engine = fallback_order.get(index + 1);
                    tracing::warn!(
                        engine = %candidate,
                        timeout_seconds = timeout.as_secs(),
                        fallback = next_engine.map(ToString::to_string),
                        "OCR model timed out"
                    );
                }
            }
        }

        anyhow::bail!(
            "OCR timed out for all attempted engines: {}",
            timed_out.join(", ")
        )
    }

    async fn ocr_with_engine(&self, encoded_image: &str, engine: Engine) -> Result<OcrOutput> {
        debug_assert_ne!(engine, Engine::Auto);

        let model = self.model_name(engine);
        let request = ChatRequest {
            model,
            messages: vec![ChatMessage {
                role: "user",
                content: OCR_PROMPT,
                images: vec![encoded_image.to_owned()],
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

fn fallback_order(engine: Engine) -> [Engine; 3] {
    match engine {
        Engine::Auto | Engine::Paddle => [Engine::Paddle, Engine::Glm, Engine::Qwen],
        Engine::Glm => [Engine::Glm, Engine::Paddle, Engine::Qwen],
        Engine::Qwen => [Engine::Qwen, Engine::Glm, Engine::Paddle],
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
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use axum::{Json, Router, extract::State, routing::post};
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::{OcrEngine, fallback_order, model_names_match};
    use crate::{config::Config, models::Engine};

    #[derive(Deserialize)]
    struct TestChatRequest {
        model: String,
    }

    #[derive(Clone, Default)]
    struct ConcurrencyState {
        current: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    async fn delayed_paddle_chat(
        State(calls): State<Arc<Mutex<Vec<String>>>>,
        Json(request): Json<TestChatRequest>,
    ) -> Json<Value> {
        calls
            .lock()
            .expect("calls lock poisoned")
            .push(request.model.clone());
        if request.model == "paddle-test" {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Json(json!({
            "message": {"content": format!("{} output", request.model)}
        }))
    }

    async fn tracked_chat(
        State(state): State<ConcurrencyState>,
        Json(request): Json<TestChatRequest>,
    ) -> Json<Value> {
        let current = state.current.fetch_add(1, Ordering::SeqCst) + 1;
        state.peak.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        state.current.fetch_sub(1, Ordering::SeqCst);
        Json(json!({
            "message": {"content": format!("{} output", request.model)}
        }))
    }

    #[test]
    fn latest_tag_is_optional_when_matching_names() {
        assert!(model_names_match("glm-ocr:latest", "glm-ocr"));
        assert!(model_names_match("glm-ocr", "glm-ocr:latest"));
        assert!(!model_names_match("glm-ocr:v2", "glm-ocr"));
    }

    #[test]
    fn fallback_orders_keep_requested_engine_first() {
        assert_eq!(
            fallback_order(Engine::Auto),
            [Engine::Paddle, Engine::Glm, Engine::Qwen]
        );
        assert_eq!(
            fallback_order(Engine::Glm),
            [Engine::Glm, Engine::Paddle, Engine::Qwen]
        );
        assert_eq!(
            fallback_order(Engine::Qwen),
            [Engine::Qwen, Engine::Glm, Engine::Paddle]
        );
    }

    #[tokio::test]
    async fn auto_falls_back_to_glm_when_paddle_times_out() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/api/chat", post(delayed_paddle_chat))
            .with_state(calls.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let config = Config {
            ollama_url: format!("http://{address}"),
            paddle_model: "paddle-test".to_owned(),
            glm_model: "glm-test".to_owned(),
            qwen_model: "qwen-test".to_owned(),
            request_timeout: Duration::from_secs(1),
            paddle_timeout: Duration::from_millis(20),
            glm_timeout: Duration::from_millis(200),
            qwen_timeout: Duration::from_millis(200),
            ..Config::default()
        };
        let engine = OcrEngine::new(Arc::new(config)).expect("engine should initialize");

        let output = engine
            .ocr_image(b"image", Engine::Auto)
            .await
            .expect("GLM fallback should succeed");

        assert_eq!(output.engine, Engine::Glm);
        assert_eq!(output.markdown, "glm-test output");
        assert_eq!(
            *calls.lock().expect("calls lock poisoned"),
            vec!["paddle-test", "glm-test"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn model_request_limit_keeps_queue_time_outside_page_timeout() {
        let state = ConcurrencyState::default();
        let app = Router::new()
            .route("/api/chat", post(tracked_chat))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test server should run");
        });

        let config = Config {
            ollama_url: format!("http://{address}"),
            paddle_model: "paddle-test".to_owned(),
            request_timeout: Duration::from_secs(1),
            paddle_timeout: Duration::from_millis(200),
            max_concurrent_model_requests: 1,
            ..Config::default()
        };
        let engine = OcrEngine::new(Arc::new(config)).expect("engine should initialize");

        let (first, second) = tokio::join!(
            engine.ocr_image(b"first", Engine::Auto),
            engine.ocr_image(b"second", Engine::Auto)
        );

        assert_eq!(
            first.expect("first OCR should succeed").engine,
            Engine::Paddle
        );
        assert_eq!(
            second.expect("second OCR should succeed").engine,
            Engine::Paddle
        );
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
