use std::str::FromStr;

use axum::routing::get;
use axum::{Json, Router, http::HeaderMap, routing::post};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use axum::{extract::State, http::StatusCode, response::IntoResponse};
use ocr_service::models::Engine;
use ocr_service::{config::Config, ocr::OcrEngine};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{sync::Arc, time::Duration};
use tower::ServiceExt;

async fn server(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

#[test]
fn callers_can_explicitly_select_jina() {
    assert_eq!(Engine::from_str("jina").unwrap().to_string(), "jina");
}

#[test]
fn disabled_cloud_does_not_restrict_local_timeout_configuration() {
    let config = Config {
        jina_timeout: Duration::from_secs(999),
        ..Config::default()
    };
    assert!(config.validate().is_ok());
    let enabled = Config {
        jina_api_key: Some("fixture-key".into()),
        ..config
    };
    assert!(enabled.validate().is_err());
}

#[tokio::test]
async fn jina_uses_its_own_authenticated_endpoint_and_returns_markdown() {
    let app = Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap, Json(body): Json<Value>| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-key");
            assert_eq!(body["model"], "jina-ocr-v1");
            assert_eq!(body["max_completion_tokens"], 8192);
            assert_eq!(body["stream"], false);
            assert!(
                body["messages"][0]["content"][1]["image_url"]["url"]
                    .as_str()
                    .unwrap()
                    .starts_with("data:image/png;base64,")
            );
            Json(json!({"choices":[{"message":{"content":"# 配料\n牛奶"},"finish_reason":"stop"}]}))
        }),
    );
    let (url, task) = server(app).await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: url,
        inference_url: "http://127.0.0.1:1".into(),
        jina_timeout: Duration::from_secs(2),
        ..Config::default()
    };
    let result = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Jina)
        .await
        .unwrap();
    assert_eq!(result.engine, Engine::Jina);
    assert_eq!(result.markdown, "# 配料\n牛奶");
    task.abort();
}

#[tokio::test]
async fn truncated_jina_output_falls_back_to_local_glm() {
    let (cloud, cloud_task) = server(Router::new().route("/v1/chat/completions", post(|| async {
        Json(json!({"choices":[{"message":{"content":"unfinished table"},"finish_reason":"length"}]}))
    }))).await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|Json(body): Json<Value>| async move {
            assert_eq!(body["model"], "glm-ocr");
            Json(json!({"choices":[{"message":{"content":"complete local output"}}]}))
        }),
    ))
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        ..Config::default()
    };
    let result = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Jina)
        .await
        .unwrap();
    assert_eq!(result.engine, Engine::Glm);
    assert_eq!(result.markdown, "complete local output");
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn cold_start_retries_once_using_retry_after() {
    let count = Arc::new(AtomicUsize::new(0));
    let (url, task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    if count.fetch_add(1, Ordering::SeqCst) == 0 {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [("retry-after", "0")],
                            "warming",
                        )
                            .into_response();
                    }
                    Json(
                        json!({"choices":[{"message":{"content":"ready"},"finish_reason":"stop"}]}),
                    )
                    .into_response()
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: url,
        inference_url: "http://127.0.0.1:1".into(),
        jina_timeout: Duration::from_secs(2),
        ..Config::default()
    };
    let result = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Jina)
        .await
        .unwrap();
    assert_eq!(result.markdown, "ready");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn health_remains_ready_when_only_jina_is_reachable() {
    let (url, task) = server(Router::new().route(
        "/v1/models/jina-ocr-v1",
        get(|| async { Json(json!({"id":"jina-ai/jina-ocr-v1"})) }),
    ))
    .await;
    let app = ocr_service::http::router(Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: url,
        inference_url: "http://127.0.0.1:1".into(),
        ..Config::default()
    })
    .unwrap();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/ocr/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["backend_ready"], false);
    assert_eq!(body["jina"]["configured"], true);
    assert_eq!(body["jina"]["reachable"], true);
    assert_eq!(body["models"][3]["available"], true);
    task.abort();
}

#[tokio::test]
async fn local_auto_does_not_upload_to_jina() {
    let count = Arc::new(AtomicUsize::new(0));
    let (cloud, cloud_task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"choices":[]}))
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { Json(json!({"choices":[{"message":{"content":"local text"}}]})) }),
    ))
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        ..Config::default()
    };
    let result = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Auto)
        .await
        .unwrap();
    assert_eq!(result.engine, Engine::Paddle);
    assert_eq!(count.load(Ordering::SeqCst), 0);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn missing_key_is_a_clear_http_configuration_error() {
    let app = ocr_service::http::router(Config::default()).unwrap();
    let data = "--fixture\r\nContent-Disposition: form-data; name=\"engine\"\r\n\r\njina\r\n--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"image.png\"\r\nContent-Type: image/png\r\n\r\nimage\r\n--fixture--\r\n";
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
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), 65536).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("OCR_JINA_API_KEY"));
}

#[tokio::test]
async fn cloud_errors_are_bounded_redacted_and_use_local_fallback() {
    for (status, expected_calls) in [
        (400, 1),
        (401, 1),
        (403, 1),
        (404, 1),
        (429, 2),
        (500, 2),
        (502, 2),
        (503, 2),
        (504, 2),
    ] {
        let count = Arc::new(AtomicUsize::new(0));
        let (cloud, cloud_task) = server(
            Router::new()
                .route(
                    "/v1/chat/completions",
                    post(move |State(count): State<Arc<AtomicUsize>>| async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::from_u16(status).unwrap(),
                            [("retry-after", "0")],
                            "fixture-key echoed document",
                        )
                    }),
                )
                .with_state(count.clone()),
        )
        .await;
        let (local, local_task) = server(Router::new().route(
            "/v1/chat/completions",
            post(|| async { Json(json!({"choices":[{"message":{"content":"local recovery"}}]})) }),
        ))
        .await;
        let config = Config {
            jina_api_key: Some("fixture-key".into()),
            jina_base_url: cloud,
            inference_url: local,
            ..Config::default()
        };
        let result = OcrEngine::new(Arc::new(config))
            .unwrap()
            .ocr_image(b"image", Engine::Jina)
            .await
            .unwrap();
        assert_eq!(result.markdown, "local recovery");
        assert_eq!(result.engine, Engine::Glm);
        assert_eq!(count.load(Ordering::SeqCst), expected_calls);
        cloud_task.abort();
        local_task.abort();
    }
}

#[tokio::test]
async fn exhausted_local_strategies_never_fall_back_to_cloud() {
    let count = Arc::new(AtomicUsize::new(0));
    let (cloud, cloud_task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"choices":[{"message":{"content":"unexpected cloud"}}]}))
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { (StatusCode::BAD_REQUEST, "not installed") }),
    ))
    .await;
    let engine = OcrEngine::new(Arc::new(Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        ..Config::default()
    }))
    .unwrap();
    for strategy in [Engine::Auto, Engine::Paddle, Engine::Glm, Engine::Qwen] {
        assert!(engine.ocr_image(b"image", strategy).await.is_err());
    }
    assert_eq!(count.load(Ordering::SeqCst), 0);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn slow_cloud_generation_times_out_without_resubmitting_the_page() {
    let count = Arc::new(AtomicUsize::new(0));
    let (cloud, cloud_task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Json(json!({"choices":[{"message":{"content":"late"}}]}))
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { Json(json!({"choices":[{"message":{"content":"local recovery"}}]})) }),
    ))
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        jina_timeout: Duration::from_millis(50),
        ..Config::default()
    };
    let result = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Jina)
        .await
        .unwrap();
    assert_eq!(result.engine, Engine::Glm);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn cloud_wait_and_request_share_one_page_budget() {
    let count = Arc::new(AtomicUsize::new(0));
    let (cloud, cloud_task) = server(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(|State(count): State<Arc<AtomicUsize>>| async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        [("retry-after", "60")],
                        "warming",
                    )
                }),
            )
            .with_state(count.clone()),
    )
    .await;
    let (local, local_task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async { Json(json!({"choices":[{"message":{"content":"local recovery"}}]})) }),
    ))
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: local,
        jina_timeout: Duration::from_millis(25),
        ..Config::default()
    };
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        OcrEngine::new(Arc::new(config))
            .unwrap()
            .ocr_image(b"image", Engine::Jina),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.engine, Engine::Glm);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    cloud_task.abort();
    local_task.abort();
}

#[tokio::test]
async fn invalid_empty_or_unauthorized_cloud_output_never_leaks_upstream_data() {
    for body in [
        "{",
        "{\"choices\":[]}",
        "{\"choices\":[{\"message\":{\"content\":\"  \"}}]}",
    ] {
        let (cloud, cloud_task) = server(Router::new().route(
            "/v1/chat/completions",
            post(move || async move { ([("content-type", "application/json")], body) }),
        ))
        .await;
        let config = Config {
            jina_api_key: Some("fixture-key".into()),
            jina_base_url: cloud,
            inference_url: "http://127.0.0.1:1".into(),
            ..Config::default()
        };
        let error = OcrEngine::new(Arc::new(config))
            .unwrap()
            .ocr_image(b"image", Engine::Jina)
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("fixture-key"));
        cloud_task.abort();
    }
    let (cloud, task) = server(Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                "fixture-key echoed private document",
            )
        }),
    ))
    .await;
    let config = Config {
        jina_api_key: Some("fixture-key".into()),
        jina_base_url: cloud,
        inference_url: "http://127.0.0.1:1".into(),
        ..Config::default()
    };
    let error = OcrEngine::new(Arc::new(config))
        .unwrap()
        .ocr_image(b"image", Engine::Jina)
        .await
        .unwrap_err();
    assert!(!format!("{error:#}").contains("private document"));
    assert!(!format!("{error:#}").contains("fixture-key"));
    task.abort();
}

fn pdf_fixture() -> Vec<u8> {
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

#[tokio::test]
async fn pdf_page_selection_and_mixed_cloud_local_merge_use_the_existing_pipeline() {
    for pages in [None, Some("2")] {
        let count = Arc::new(AtomicUsize::new(0));
        let (cloud, cloud_task) = server(Router::new().route("/v1/chat/completions", post(|State(count): State<Arc<AtomicUsize>>, Json(body): Json<Value>| async move {
            assert!(body["messages"][0]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,"));
            let content = if count.fetch_add(1, Ordering::SeqCst) == 0 { "cloud page" } else { "" };
            Json(json!({"choices":[{"message":{"content":content},"finish_reason":"stop"}]}))
        })).with_state(count.clone())).await;
        let (local, local_task) = server(Router::new().route(
            "/v1/chat/completions",
            post(|| async { Json(json!({"choices":[{"message":{"content":"local page"}}]})) }),
        ))
        .await;
        let app = ocr_service::http::router(Config {
            jina_api_key: Some("fixture-key".into()),
            jina_base_url: cloud,
            inference_url: local,
            pdf_dpi: 36,
            ..Config::default()
        })
        .unwrap();
        let mut data =
            b"--fixture\r\nContent-Disposition: form-data; name=\"engine\"\r\n\r\njina\r\n"
                .to_vec();
        if let Some(pages) = pages {
            data.extend_from_slice(format!("--fixture\r\nContent-Disposition: form-data; name=\"page_range\"\r\n\r\n{pages}\r\n").as_bytes());
        }
        data.extend_from_slice(b"--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"test.pdf\"\r\nContent-Type: application/pdf\r\n\r\n");
        data.extend_from_slice(&pdf_fixture());
        data.extend_from_slice(b"\r\n--fixture--\r\n");
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
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["pages"], if pages.is_some() { 1 } else { 2 });
        assert_eq!(
            body["engine"],
            if pages.is_some() { "jina" } else { "mixed" }
        );
        assert_eq!(
            count.load(Ordering::SeqCst),
            if pages.is_some() { 1 } else { 2 }
        );
        assert!(body["markdown"].as_str().unwrap().contains("cloud page"));
        cloud_task.abort();
        local_task.abort();
    }
}

#[tokio::test]
async fn skill_can_explicitly_upload_to_jina_through_http() {
    let (url, task) = server(Router::new().route(
        "/v1/ocr/image",
        post(|request: axum::extract::Multipart| async move {
            let mut request = request;
            let mut engine = String::new();
            while let Some(field) = request.next_field().await.unwrap() {
                if field.name() == Some("engine") {
                    engine = field.text().await.unwrap();
                }
            }
            assert_eq!(engine, "jina");
            Json(json!({"markdown":"skill result", "engine":"jina", "pages":1, "duration_ms":1}))
        }),
    ))
    .await;
    let fixture = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(fixture.path(), b"image").unwrap();
    let output = tokio::process::Command::new("bash")
        .arg(".agents/skills/ocr-local-service/scripts/ocr.sh")
        .arg("image")
        .arg(fixture.path())
        .args(["--engine", "jina"])
        .env("OCR_SERVICE_URL", url)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "skill result"
    );
    task.abort();
}
