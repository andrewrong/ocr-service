use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use tokio::sync::Semaphore;

use crate::{
    config::Config,
    models::{Engine, JinaHealth, ModelStatus},
    ocr::backend::OpenAiCompatibleBackend,
    ocr::jina::{JinaBackend, JinaNotConfigured, MODEL},
};

const OCR_PROMPT: &str = "Transcribe this document image faithfully into Markdown. Preserve headings, paragraphs, lists, tables, formulas, reading order, and line breaks where meaningful. Do not summarize, explain, or wrap the result in a code fence. Return only the transcription.";

#[derive(Clone)]
pub struct OcrEngine {
    backend: OpenAiCompatibleBackend,
    jina: JinaBackend,
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
        let backend = OpenAiCompatibleBackend::new(&config)?;
        let request_slots = Arc::new(Semaphore::new(config.max_concurrent_model_requests));
        Ok(Self {
            backend,
            jina: JinaBackend::new(&config)?,
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
            Engine::Jina => MODEL,
            Engine::Auto => unreachable!("auto is resolved above"),
        }
    }

    pub async fn ocr_image(&self, image: &[u8], engine: Engine) -> Result<OcrOutput> {
        self.ocr_image_traced(image, engine, 1, &AtomicBool::new(false))
            .await
    }

    /// Like `ocr_image`, and sets `cloud_attempted` before any request that may reach Jina.
    /// `page` only labels the attempt log lines.
    pub async fn ocr_image_traced(
        &self,
        image: &[u8],
        engine: Engine,
        page: usize,
        cloud_attempted: &AtomicBool,
    ) -> Result<OcrOutput> {
        anyhow::ensure!(!image.is_empty(), "image is empty");
        if engine == Engine::Jina && !self.jina.configured() {
            return Err(JinaNotConfigured.into());
        }

        let image_data_url = format!(
            "data:{};base64,{}",
            image_media_type(image),
            STANDARD.encode(image)
        );
        let mut fallback_order = fallback_order(engine);
        if engine != Engine::Jina && self.config.jina_fallback_on_local_failure {
            fallback_order.push(Engine::Jina);
        }
        let mut failures = Vec::new();
        let mut attempted_models = HashSet::new();
        let candidates = fallback_order
            .into_iter()
            .filter(|candidate| {
                attempted_models.insert((*candidate == Engine::Jina, self.model_name(*candidate)))
            })
            .collect::<Vec<_>>();

        for (index, candidate) in candidates.iter().copied().enumerate() {
            let timeout = self.config.timeout_for(candidate);
            let permit = self
                .request_slots
                .acquire()
                .await
                .context("OCR model request limiter closed")?;
            if candidate == Engine::Jina {
                cloud_attempted.store(true, Ordering::SeqCst);
            }
            let started = Instant::now();
            let attempt =
                tokio::time::timeout(timeout, self.ocr_with_engine(&image_data_url, candidate))
                    .await;
            let duration_ms = started.elapsed().as_millis() as u64;
            drop(permit);
            let (outcome, chars) = match &attempt {
                Ok(Ok(output)) => ("ok", Some(output.markdown.chars().count())),
                Ok(Err(_)) => ("error", None),
                Err(_) => ("timeout", None),
            };
            tracing::info!(
                engine = %candidate,
                page,
                image_bytes = image.len(),
                duration_ms,
                outcome,
                chars,
                "OCR attempt"
            );

            match attempt {
                Ok(Ok(output)) => return Ok(output),
                Ok(Err(error)) => {
                    failures.push(format!("{candidate}: {error:#}"));
                    let next_engine = candidates.get(index + 1);
                    tracing::warn!(
                        engine = %candidate,
                        %error,
                        fallback = next_engine.map(ToString::to_string),
                        "OCR model request failed"
                    );
                }
                Err(_) => {
                    failures.push(format!(
                        "{candidate}: timed out after {}s",
                        timeout.as_secs()
                    ));
                    let next_engine = candidates.get(index + 1);
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
            "OCR failed for all attempted engines: {}",
            failures.join("; ")
        )
    }

    async fn ocr_with_engine(&self, image_data_url: &str, engine: Engine) -> Result<OcrOutput> {
        debug_assert_ne!(engine, Engine::Auto);

        let model = self.model_name(engine);
        if engine == Engine::Jina {
            return Ok(OcrOutput {
                markdown: self.jina.transcribe(image_data_url).await?,
                engine,
            });
        }
        let transcription = self
            .backend
            .transcribe(model, OCR_PROMPT, image_data_url)
            .await?;
        if let Some(removed) = transcription.repetition_removed {
            tracing::warn!(engine = %engine, removed_chars = removed, "cut a repetition from the model output");
        }

        Ok(OcrOutput {
            markdown: transcription.markdown,
            engine,
        })
    }

    pub async fn health(&self) -> Result<Vec<ModelStatus>> {
        let model_names = self.backend.model_names().await?;
        let installed: Vec<&str> = model_names.iter().map(String::as_str).collect();

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

    /// Aggregate provider availability without invoking cloud inference.
    /// Cloud reachability does not validate the key, balance, or inference capacity.
    pub async fn health_snapshot(&self) -> (bool, Vec<ModelStatus>, JinaHealth) {
        let (local, cloud) = tokio::join!(
            tokio::time::timeout(std::time::Duration::from_secs(2), self.health()),
            tokio::time::timeout(std::time::Duration::from_secs(2), self.jina.reachable())
        );
        let (local_ready, mut models) = match local {
            Ok(Ok(models)) => (true, models),
            _ => (
                false,
                [Engine::Paddle, Engine::Glm, Engine::Qwen]
                    .into_iter()
                    .map(|engine| ModelStatus {
                        engine,
                        name: self.model_name(engine).to_owned(),
                        available: false,
                    })
                    .collect(),
            ),
        };
        let jina = JinaHealth {
            configured: self.jina.configured(),
            reachable: cloud.unwrap_or(false),
            fallback_on_local_failure: self.config.jina_fallback_on_local_failure,
        };
        models.push(ModelStatus {
            engine: Engine::Jina,
            name: MODEL.to_owned(),
            available: jina.configured && jina.reachable,
        });
        (local_ready, models, jina)
    }
}

fn image_media_type(image: &[u8]) -> &'static str {
    if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if image.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if image.starts_with(b"GIF87a") || image.starts_with(b"GIF89a") {
        "image/gif"
    } else if image.len() >= 12 && image.starts_with(b"RIFF") && &image[8..12] == b"WEBP" {
        "image/webp"
    } else if image.starts_with(b"BM") {
        "image/bmp"
    } else {
        "image/png"
    }
}

fn fallback_order(engine: Engine) -> Vec<Engine> {
    match engine {
        Engine::Auto | Engine::Paddle => vec![Engine::Paddle, Engine::Glm, Engine::Qwen],
        Engine::Glm => vec![Engine::Glm, Engine::Paddle, Engine::Qwen],
        Engine::Qwen => vec![Engine::Qwen, Engine::Glm, Engine::Paddle],
        Engine::Jina => vec![Engine::Jina, Engine::Glm, Engine::Paddle, Engine::Qwen],
    }
}

fn model_names_match(installed: &str, configured: &str) -> bool {
    installed == configured
        || installed.strip_suffix(":latest") == Some(configured)
        || configured.strip_suffix(":latest") == Some(installed)
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

    use axum::{
        Json, Router,
        extract::State,
        routing::{get, post},
    };
    use serde::Deserialize;
    use serde_json::{Value, json};

    use super::{OcrEngine, fallback_order, image_media_type, model_names_match};
    use crate::{config::Config, models::Engine};

    #[derive(Debug, Deserialize)]
    struct TestChatRequest {
        model: String,
        messages: Value,
    }

    #[derive(Clone, Default)]
    struct ConcurrencyState {
        current: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    async fn delayed_paddle_chat(
        State(calls): State<Arc<Mutex<Vec<TestChatRequest>>>>,
        Json(request): Json<TestChatRequest>,
    ) -> Json<Value> {
        let model = request.model.clone();
        calls.lock().expect("calls lock poisoned").push(request);
        if model == "paddle-test" {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Json(json!({
            "choices": [{"message": {"content": format!("{model} output")}}]
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
            "choices": [{"message": {"content": format!("{} output", request.model)}}]
        }))
    }

    async fn unavailable_paddle_chat(
        Json(request): Json<TestChatRequest>,
    ) -> impl axum::response::IntoResponse {
        if request.model == "paddle-test" {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": {"message": "model is not loaded"}})),
            );
        }
        (
            axum::http::StatusCode::OK,
            Json(json!({
                "choices": [{"message": {"content": format!("{} output", request.model)}}]
            })),
        )
    }

    async fn unavailable_chat(
        State(calls): State<Arc<AtomicUsize>>,
    ) -> impl axum::response::IntoResponse {
        calls.fetch_add(1, Ordering::SeqCst);
        (
            axum::http::StatusCode::BAD_REQUEST,
            Json(json!({"error": {"message": "model is not loaded"}})),
        )
    }

    async fn listed_models() -> Json<Value> {
        Json(json!({
            "object": "list",
            "data": [
                {"id": "paddle-test"},
                {"id": "glm-test"}
            ]
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

    #[test]
    fn detects_common_image_media_types() {
        assert_eq!(image_media_type(b"\x89PNG\r\n\x1a\nrest"), "image/png");
        assert_eq!(image_media_type(b"\xff\xd8\xffrest"), "image/jpeg");
        assert_eq!(image_media_type(b"GIF89arest"), "image/gif");
        assert_eq!(image_media_type(b"RIFFxxxxWEBPrest"), "image/webp");
    }

    #[tokio::test]
    async fn auto_falls_back_to_glm_when_paddle_times_out() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/v1/chat/completions", post(delayed_paddle_chat))
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
            inference_url: format!("http://{address}"),
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
            calls
                .lock()
                .expect("calls lock poisoned")
                .iter()
                .map(|request| request.model.as_str())
                .collect::<Vec<_>>(),
            vec!["paddle-test", "glm-test"]
        );
        let calls = calls.lock().expect("calls lock poisoned");
        assert_eq!(calls[0].messages[0]["role"], "user");
        assert_eq!(calls[0].messages[0]["content"][0]["type"], "text");
        assert!(
            calls[0].messages[0]["content"][1]["image_url"]["url"]
                .as_str()
                .expect("image URL should be a string")
                .starts_with("data:image/png;base64,")
        );
        server.abort();
    }

    #[tokio::test]
    async fn auto_falls_back_when_a_model_is_unavailable() {
        let app = Router::new().route("/v1/chat/completions", post(unavailable_paddle_chat));
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
            inference_url: format!("http://{address}"),
            paddle_model: "paddle-test".to_owned(),
            glm_model: "glm-test".to_owned(),
            qwen_model: "qwen-test".to_owned(),
            ..Config::default()
        };

        let output = OcrEngine::new(Arc::new(config))
            .expect("engine should initialize")
            .ocr_image(b"image", Engine::Auto)
            .await
            .expect("GLM fallback should succeed");

        assert_eq!(output.engine, Engine::Glm);
        assert_eq!(output.markdown, "glm-test output");
        server.abort();
    }

    /// A runtime fixture that records each request body and answers from a script of
    /// `(finish_reason, text)` pairs, one per request, repeating the last.
    async fn scripted_runtime(
        script: Vec<(&'static str, String)>,
    ) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        type Shared = (Arc<Mutex<Vec<Value>>>, Arc<Vec<(&'static str, String)>>);
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(
                    |State((bodies, script)): State<Shared>, Json(body): Json<Value>| async move {
                        let mut bodies = bodies.lock().expect("bodies lock poisoned");
                        let (reason, text) = &script[bodies.len().min(script.len() - 1)];
                        bodies.push(body);
                        Json(json!({"choices": [{
                            "message": {"content": text}, "finish_reason": reason
                        }]}))
                    },
                ),
            )
            .with_state((bodies.clone(), Arc::new(script)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server runs");
        });
        (url, bodies, task)
    }

    fn scripted_config(url: String) -> Config {
        Config {
            inference_url: url,
            paddle_model: "paddle-test".to_owned(),
            glm_model: "glm-test".to_owned(),
            qwen_model: "qwen-test".to_owned(),
            ..Config::default()
        }
    }

    #[tokio::test]
    async fn local_requests_carry_the_configured_max_tokens() {
        let (url, bodies, server) = scripted_runtime(vec![("stop", "text".to_owned())]).await;
        assert_eq!(Config::default().local_max_output_tokens, 4096);

        let engine = OcrEngine::new(Arc::new(scripted_config(url.clone()))).unwrap();
        engine.ocr_image(b"image", Engine::Glm).await.unwrap();
        let config = Config {
            local_max_output_tokens: 77,
            ..scripted_config(url)
        };
        let engine = OcrEngine::new(Arc::new(config)).unwrap();
        engine.ocr_image(b"image", Engine::Glm).await.unwrap();

        let bodies = bodies.lock().unwrap();
        assert_eq!(bodies[0]["max_tokens"], 4096);
        assert_eq!(bodies[1]["max_tokens"], 77);
        server.abort();
    }

    #[tokio::test]
    async fn repetition_at_the_limit_is_cut_to_its_first_copy() {
        let looped = format!(
            "alpha\nbeta\ngamma\n{}alpha\nbe",
            "alpha\nbeta\ngamma\n".repeat(40)
        );
        let (url, bodies, server) = scripted_runtime(vec![("length", looped)]).await;

        let output = OcrEngine::new(Arc::new(scripted_config(url)))
            .unwrap()
            .ocr_image(b"image", Engine::Glm)
            .await
            .expect("cut repetition succeeds");

        assert_eq!(output.engine, Engine::Glm);
        assert_eq!(output.markdown, "alpha\nbeta\ngamma");
        assert_eq!(bodies.lock().unwrap().len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn incomplete_third_copy_is_cut_without_fallback() {
        let (url, bodies, server) = scripted_runtime(vec![
            (
                "length",
                "alpha\nbeta\ngamma\nalpha\nbeta\ngamma\nalpha\nbe".to_owned(),
            ),
            ("stop", "fallback output".to_owned()),
        ])
        .await;

        let output = OcrEngine::new(Arc::new(scripted_config(url)))
            .unwrap()
            .ocr_image(b"image", Engine::Glm)
            .await
            .unwrap();

        let requests = bodies.lock().unwrap().len();
        server.abort();

        assert_eq!(output.engine, Engine::Glm);
        assert_eq!(output.markdown, "alpha\nbeta\ngamma");
        assert_eq!(requests, 1);
    }

    #[tokio::test]
    async fn truncation_without_repetition_falls_back() {
        let (url, bodies, server) = scripted_runtime(vec![
            ("length", "one\ntwo\nthree\nfour".to_owned()),
            ("stop", "second engine".to_owned()),
        ])
        .await;

        let output = OcrEngine::new(Arc::new(scripted_config(url)))
            .unwrap()
            .ocr_image(b"image", Engine::Glm)
            .await
            .expect("next engine answers");

        assert_eq!(output.engine, Engine::Paddle);
        assert_eq!(output.markdown, "second engine");
        assert_eq!(bodies.lock().unwrap().len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn repetition_below_the_limit_is_returned_unchanged() {
        let text = "same line\n".repeat(20).trim().to_owned();
        let (url, _bodies, server) = scripted_runtime(vec![("stop", text.clone())]).await;

        let output = OcrEngine::new(Arc::new(scripted_config(url)))
            .unwrap()
            .ocr_image(b"image", Engine::Glm)
            .await
            .unwrap();

        assert_eq!(output.markdown, text);
        server.abort();
    }

    #[tokio::test]
    async fn fallback_does_not_retry_the_same_model_id() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1/chat/completions", post(unavailable_chat))
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
            inference_url: format!("http://{address}"),
            paddle_model: "vision-test".to_owned(),
            glm_model: "vision-test".to_owned(),
            qwen_model: "vision-test".to_owned(),
            ..Config::default()
        };

        let error = OcrEngine::new(Arc::new(config))
            .expect("engine should initialize")
            .ocr_image(b"image", Engine::Auto)
            .await
            .expect_err("unavailable model should fail");

        assert!(error.to_string().contains("model is not loaded"));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn model_request_limit_keeps_queue_time_outside_page_timeout() {
        let state = ConcurrencyState::default();
        let app = Router::new()
            .route("/v1/chat/completions", post(tracked_chat))
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
            inference_url: format!("http://{address}"),
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

    #[tokio::test]
    async fn health_uses_openai_compatible_model_listing() {
        let app = Router::new().route("/v1/models", get(listed_models));
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
            inference_url: format!("http://{address}"),
            paddle_model: "paddle-test".to_owned(),
            glm_model: "glm-test".to_owned(),
            qwen_model: "qwen-test".to_owned(),
            ..Config::default()
        };

        let models = OcrEngine::new(Arc::new(config))
            .expect("engine should initialize")
            .health()
            .await
            .expect("model listing should parse");

        assert!(models[0].available);
        assert!(models[1].available);
        assert!(!models[2].available);
        server.abort();
    }
}
