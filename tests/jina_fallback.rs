use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use ocr_service::config::Config;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

/// A Jina fixture that answers every request with `text` and counts them.
async fn jina_fixture(
    text: &'static str,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let (url, task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(move |State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}]}))
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    (url, count, task)
}

/// A runtime fixture that rejects every request.
async fn failing_runtime() -> (String, tokio::task::JoinHandle<()>) {
    server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { (StatusCode::BAD_REQUEST, "model is not loaded") }),
    ))
    .await
}

fn config(cloud: String, local: String, fallback: bool) -> Config {
    Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        jina_fallback_on_local_failure: fallback,
        ..Config::default()
    }
}

async fn post_image(app: Router, engine: &str) -> (StatusCode, Value) {
    let data = format!(
        "--fixture\r\nContent-Disposition: form-data; name=\"engine\"\r\n\r\n{engine}\r\n--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\nimage\r\n--fixture--\r\n"
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
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn auto_falls_back_to_jina_once_after_every_local_engine_failed() {
    let (cloud, count, cloud_task) = jina_fixture("text from jina").await;
    let (local, local_task) = failing_runtime().await;
    let app = ocr_service::http::router(config(cloud, local, true)).unwrap();

    let (status, body) = post_image(app, "auto").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["markdown"], "text from jina");
    assert_eq!(body["engine"], "jina");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn every_local_strategy_may_fall_back_to_jina_when_enabled() {
    let (cloud, count, cloud_task) = jina_fixture("text from jina").await;
    let (local, local_task) = failing_runtime().await;
    for (index, strategy) in ["paddle", "glm", "qwen"].into_iter().enumerate() {
        let app = ocr_service::http::router(config(cloud.clone(), local.clone(), true)).unwrap();
        let (status, body) = post_image(app, strategy).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["engine"], "jina");
        assert_eq!(count.load(Ordering::SeqCst), index + 1);
    }
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn auto_never_reaches_jina_when_the_fallback_is_off() {
    let (cloud, count, cloud_task) = jina_fixture("text from jina").await;
    let (local, local_task) = failing_runtime().await;
    let app = ocr_service::http::router(config(cloud, local, false)).unwrap();

    let (status, _) = post_image(app, "auto").await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn a_local_success_never_reaches_jina_when_the_fallback_is_on() {
    let (cloud, count, cloud_task) = jina_fixture("text from jina").await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { Json(json!({"choices":[{"message":{"content":"local text"}}]})) }),
    ))
    .await;
    let app = ocr_service::http::router(config(cloud, local, true)).unwrap();

    let (status, body) = post_image(app, "auto").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "paddle");
    assert_eq!(count.load(Ordering::SeqCst), 0);
    cloud_task.abort();
    local_task.abort();
}

#[test]
fn fallback_without_a_jina_key_fails_validation() {
    let enabled = Config {
        jina_fallback_on_local_failure: true,
        ..Config::default()
    };
    assert!(enabled.validate().is_err());
    let keyed = Config {
        jina_api_key: Some("fixture-key".into()),
        ..enabled
    };
    assert!(keyed.validate().is_ok());
    assert!(!Config::default().jina_fallback_on_local_failure);
}

#[tokio::test]
async fn error_response_says_whether_jina_was_tried() {
    // Jina tried: the fallback is on and the Jina fixture fails too.
    let (local, local_task) = failing_runtime().await;
    let (failing_cloud, cloud_task) = failing_runtime().await;
    let app =
        ocr_service::http::router(config(failing_cloud.clone(), local.clone(), true)).unwrap();
    let (status, body) = post_image(app, "auto").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["cloud_attempted"], true);

    // Explicit jina that fails also tried Jina.
    let app =
        ocr_service::http::router(config(failing_cloud.clone(), local.clone(), false)).unwrap();
    let (_, body) = post_image(app, "jina").await;
    assert_eq!(body["cloud_attempted"], true);

    // Jina not tried: the fallback is off.
    let app = ocr_service::http::router(config(failing_cloud, local.clone(), false)).unwrap();
    let (status, body) = post_image(app, "auto").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["cloud_attempted"], false);

    // Bad request: nothing was sent anywhere.
    let app = ocr_service::http::router(Config::default()).unwrap();
    let (status, body) = post_image(app, "bogus").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["cloud_attempted"], false);
    let (status, body) = post_image(
        ocr_service::http::router(Config::default()).unwrap(),
        "jina",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["cloud_attempted"], false);
    local_task.abort();
    cloud_task.abort();
}

#[tokio::test]
async fn health_reports_the_fallback_setting() {
    for enabled in [true, false] {
        let (local, local_task) = server(Router::new().route(
            "/v1/models",
            get(|| async { Json(json!({"object":"list","data":[]})) }),
        ))
        .await;
        let (cloud, cloud_task) = server(Router::new().route(
            "/v1/models/jina-ocr-v1",
            get(|| async { Json(json!({"id": "jina-ocr-v1"})) }),
        ))
        .await;
        let app = ocr_service::http::router(config(cloud, local, enabled)).unwrap();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/ocr/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 65536).await.unwrap();
        let health: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(health["jina"]["fallback_on_local_failure"], enabled);
        cloud_task.abort();
        local_task.abort();
    }
}

#[tokio::test]
async fn extractor_rejection_reports_cloud_not_attempted() {
    let app = ocr_service::http::router(Config::default()).unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ocr/image")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let payload: Value = serde_json::from_slice(&bytes).expect("error must follow JSON contract");
    assert_eq!(payload["cloud_attempted"], false);
}

#[test]
fn fallback_rejects_blank_jina_keys() {
    for key in ["", "   "] {
        let config = Config {
            jina_fallback_on_local_failure: true,
            jina_api_key: Some(key.to_owned()),
            ..Config::default()
        };

        assert!(
            config.validate().is_err(),
            "fallback must reject a blank Jina key"
        );
    }
}

fn two_page_pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] /Resources <<>> /Contents 5 0 R >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 72 72] /Resources <<>> /Contents 6 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", index + 1));
    }
    let xref = pdf.len();
    pdf.push_str("xref\n0 7\n0000000000 65535 f \n");
    for offset in offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
    ));
    pdf.into_bytes()
}

async fn post_pdf(app: Router) -> (StatusCode, Value) {
    let mut data = b"--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.pdf\"\r\nContent-Type: application/pdf\r\n\r\n".to_vec();
    data.extend(two_page_pdf());
    data.extend(b"\r\n--fixture--\r\n");
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/ocr/pdf")
                .header("content-type", "multipart/form-data; boundary=fixture")
                .body(Body::from(data))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn pdf_error_keeps_the_jina_attempt_made_for_an_earlier_page() {
    // Jina answers the first page it is given and rejects the next one, so the request fails
    // after one page was already uploaded.
    let count = Arc::new(AtomicUsize::new(0));
    let (cloud, cloud_task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        return Json(json!({"choices":[{"message":{"content":"page text"},"finish_reason":"stop"}]}))
                            .into_response();
                    }
                    (StatusCode::BAD_REQUEST, "rejected").into_response()
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let (local, local_task) = failing_runtime().await;
    let app = ocr_service::http::router(config(cloud, local, true)).unwrap();

    let (status, body) = post_pdf(app).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["cloud_attempted"], true);
    assert_eq!(count.load(Ordering::SeqCst), 2);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn pdf_error_without_the_fallback_reports_no_cloud_attempt() {
    let (cloud, count, cloud_task) = jina_fixture("text from jina").await;
    let (local, local_task) = failing_runtime().await;
    let app = ocr_service::http::router(config(cloud, local, false)).unwrap();

    let (status, body) = post_pdf(app).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert_eq!(body["cloud_attempted"], false);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    cloud_task.abort();
    local_task.abort();
}
