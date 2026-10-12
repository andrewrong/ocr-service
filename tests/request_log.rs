use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::Request as ExtractRequest,
    http::{Request, StatusCode},
    routing::post,
};
use ocr_service::{config::Config, http::router};
use serde_json::{Value, json};
use tower::ServiceExt;
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*, registry::LookupSpan};

type Fields = BTreeMap<String, Value>;

/// Collects the fields of every `info` event this crate emits.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Fields>>>);

struct Recorder<'a>(&'a mut Fields);

impl Visit for Recorder<'_> {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        if *meta.level() != Level::INFO || !meta.target().starts_with("ocr_service") {
            return;
        }
        let mut fields = Fields::new();
        event.record(&mut Recorder(&mut fields));
        self.0.lock().unwrap().push(fields);
    }
}

async fn runtime(handler: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, handler).await.unwrap() });
    url
}

async fn answering(text: &'static str) -> String {
    runtime(Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            Json(json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}))
        }),
    ))
    .await
}

fn config(url: String) -> Config {
    Config {
        inference_url: url,
        paddle_model: "paddle-test".into(),
        glm_model: "glm-test".into(),
        qwen_model: "qwen-test".into(),
        ..Config::default()
    }
}

/// Posts an image upload and returns the status with the info events it logged.
async fn upload(config: Config, filename: &str, engine: &str) -> (StatusCode, Vec<Fields>) {
    let capture = Capture::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(capture.clone()));
    let app = router(config).unwrap();
    let data = format!(
        "--fixture\r\nContent-Disposition: form-data; name=\"engine\"\r\n\r\n{engine}\r\n--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: image/png\r\n\r\nimage\r\n--fixture--\r\n"
    );
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ocr/image")
                .header("content-type", "multipart/form-data; boundary=fixture")
                .body(Body::from(data))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    to_bytes(response.into_body(), 65536).await.unwrap();
    let events = capture.0.lock().unwrap().clone();
    (status, events)
}

fn with_message<'a>(events: &'a [Fields], message: &str) -> Vec<&'a Fields> {
    events
        .iter()
        .filter(|fields| fields.get("message") == Some(&json!(message)))
        .collect()
}

#[tokio::test]
async fn a_successful_image_request_logs_one_attempt_and_one_request() {
    let url = answering("hello").await;
    let (status, events) = upload(config(url), "scan.png", "glm").await;
    assert_eq!(status, StatusCode::OK);

    let attempts = with_message(&events, "OCR attempt");
    assert_eq!(attempts.len(), 1, "{events:?}");
    let attempt = attempts[0];
    assert_eq!(attempt["engine"], "glm");
    assert_eq!(attempt["page"], 1);
    assert_eq!(attempt["image_bytes"], 5);
    assert!(attempt["duration_ms"].is_u64());
    assert_eq!(attempt["outcome"], "ok");
    assert_eq!(attempt["chars"], 5);

    let requests = with_message(&events, "OCR request");
    assert_eq!(requests.len(), 1, "{events:?}");
    let request = requests[0];
    assert_eq!(request["route"], "/v1/ocr/image");
    assert_eq!(request["requested_engine"], "glm");
    assert_eq!(request["upload_bytes"], 5);
    assert_eq!(request["pages"], 1);
    assert_eq!(request["engine"], "glm");
    assert_eq!(request["status"], 200);
    assert!(request["duration_ms"].is_u64());
}

#[tokio::test]
async fn a_timed_out_engine_and_its_fallback_each_log_an_attempt() {
    let url = runtime(Router::new().route(
        "/v1/chat/completions",
        post(|request: ExtractRequest| async move {
            let body = to_bytes(request.into_body(), usize::MAX).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            if body["model"] == "paddle-test" {
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Json(json!({"choices":[{"message":{"content":"done"},"finish_reason":"stop"}]}))
        }),
    ))
    .await;
    let config = Config {
        request_timeout: Duration::from_secs(5),
        paddle_timeout: Duration::from_millis(30),
        glm_timeout: Duration::from_secs(2),
        ..config(url)
    };
    let (status, events) = upload(config, "scan.png", "auto").await;
    assert_eq!(status, StatusCode::OK);

    let attempts = with_message(&events, "OCR attempt");
    assert_eq!(attempts.len(), 2, "{events:?}");
    assert_eq!(attempts[0]["engine"], "paddle");
    assert_eq!(attempts[0]["outcome"], "timeout");
    assert!(!attempts[0].contains_key("chars"));
    assert_eq!(attempts[1]["engine"], "glm");
    assert_eq!(attempts[1]["outcome"], "ok");

    let requests = with_message(&events, "OCR request");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["requested_engine"], "auto");
    assert_eq!(requests[0]["engine"], "glm");
}

#[tokio::test]
async fn a_request_failing_on_every_engine_logs_its_status_and_no_engine() {
    let url = runtime(Router::new().route(
        "/v1/chat/completions",
        post(|| async { (StatusCode::BAD_REQUEST, "model is not loaded") }),
    ))
    .await;
    let (status, events) = upload(config(url), "scan.png", "auto").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);

    let attempts = with_message(&events, "OCR attempt");
    assert_eq!(attempts.len(), 3, "{events:?}");
    assert!(attempts.iter().all(|attempt| attempt["outcome"] == "error"));

    let requests = with_message(&events, "OCR request");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["status"], 502);
    assert!(!requests[0].contains_key("engine"));
}

#[tokio::test]
async fn no_event_holds_a_file_name_or_the_recognized_text() {
    let url = answering("TEXT-MARKER-7c1e").await;
    let (status, events) = upload(config(url), "NAME-MARKER-4f2a.png", "glm").await;
    assert_eq!(status, StatusCode::OK);

    assert!(!events.is_empty());
    let logged = format!("{events:?}");
    assert!(!logged.contains("TEXT-MARKER-7c1e"), "{logged}");
    assert!(!logged.contains("NAME-MARKER-4f2a"), "{logged}");
}
