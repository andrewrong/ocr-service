use std::{str::FromStr, sync::Arc, time::Instant};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Multipart, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::{StreamExt, TryStreamExt, stream};
use tower_http::trace::TraceLayer;

use crate::{
    config::Config,
    models::{Engine, ErrorResponse, HealthResponse, ModelStatus, OcrResponse},
    ocr::{OcrEngine, PageRange, PdfPages, merger::merge_pages},
};

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    ocr: OcrEngine,
}

pub fn router(config: Config) -> anyhow::Result<Router> {
    let max_upload_bytes = config.max_upload_bytes;
    let config = Arc::new(config);
    let state = AppState {
        ocr: OcrEngine::new(config.clone())?,
        config,
    };

    Ok(Router::new()
        .route("/ocr/health", get(health))
        .route("/ocr/image", post(ocr_image))
        .route("/ocr/pdf", post(ocr_pdf))
        .route("/v1/ocr/health", get(health))
        .route("/v1/ocr/image", post(ocr_image))
        .route("/v1/ocr/pdf", post(ocr_pdf))
        .route("/openapi.yaml", get(openapi_contract))
        .layer(DefaultBodyLimit::max(max_upload_bytes))
        .layer(TraceLayer::new_for_http())
        .with_state(state))
}

async fn openapi_contract() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        include_str!("../../openapi.yaml"),
    )
}

async fn ocr_image(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<OcrResponse>, ApiError> {
    let started = Instant::now();
    let upload = parse_upload(multipart, false).await?;
    let output = state
        .ocr
        .ocr_image(&upload.file, upload.engine)
        .await
        .map_err(ApiError::upstream)?;

    Ok(Json(OcrResponse {
        markdown: output.markdown,
        engine: output.engine.to_string(),
        pages: 1,
        duration_ms: started.elapsed().as_millis(),
    }))
}

async fn ocr_pdf(
    State(state): State<AppState>,
    multipart: Multipart,
) -> Result<Json<OcrResponse>, ApiError> {
    let started = Instant::now();
    let upload = parse_upload(multipart, true).await?;
    let rendered = PdfPages::render(&upload.file, upload.page_range, state.config.pdf_dpi)
        .await
        .map_err(ApiError::bad_request)?;
    let page_count = rendered.pages.len();
    let concurrency = state.config.max_concurrent_pages;
    let requested_engine = upload.engine;
    let page_paths = rendered
        .pages
        .iter()
        .map(|page| (page.number, page.path.clone()))
        .collect::<Vec<_>>();

    let results = stream::iter(page_paths.into_iter().map(|(page_number, path)| {
        let ocr = state.ocr.clone();
        async move {
            let image = tokio::fs::read(&path)
                .await
                .with_context(|| format!("failed to read rendered page {page_number}"))?;
            let output = ocr.ocr_image(&image, requested_engine).await?;
            anyhow::Ok((page_number, output.markdown, output.engine))
        }
    }))
    .buffer_unordered(concurrency)
    .try_collect::<Vec<_>>()
    .await
    .map_err(ApiError::upstream)?;

    let mut pages = Vec::with_capacity(page_count);
    let mut used_engines = Vec::with_capacity(page_count);
    for (number, markdown, engine) in results {
        used_engines.push(engine);
        pages.push((number, markdown));
    }

    Ok(Json(OcrResponse {
        markdown: merge_pages(pages),
        engine: response_engine(&used_engines, state.ocr.resolved_engine(requested_engine)),
        pages: page_count,
        duration_ms: started.elapsed().as_millis(),
    }))
}

fn response_engine(used_engines: &[Engine], default: Engine) -> String {
    let first = used_engines.first().copied().unwrap_or(default);
    if used_engines.iter().all(|engine| *engine == first) {
        first.to_string()
    } else {
        "mixed".to_owned()
    }
}

async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match state.ocr.health().await {
        Ok(models) => {
            let ready = models.iter().any(|model| model.available);
            (
                if ready {
                    StatusCode::OK
                } else {
                    StatusCode::SERVICE_UNAVAILABLE
                },
                Json(HealthResponse {
                    status: if ready { "ok" } else { "degraded" },
                    ollama: true,
                    models,
                }),
            )
        }
        Err(error) => {
            tracing::warn!(%error, "Ollama health check failed");
            let models = [Engine::Paddle, Engine::Glm, Engine::Qwen]
                .into_iter()
                .map(|engine| ModelStatus {
                    engine,
                    name: state.ocr.model_name(engine).to_owned(),
                    available: false,
                })
                .collect();
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "degraded",
                    ollama: false,
                    models,
                }),
            )
        }
    }
}

struct Upload {
    file: Vec<u8>,
    engine: Engine,
    page_range: Option<PageRange>,
}

async fn parse_upload(
    mut multipart: Multipart,
    allow_page_range: bool,
) -> Result<Upload, ApiError> {
    let mut file = None;
    let mut engine = Engine::Auto;
    let mut page_range = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| ApiError::bad_request(error.into()))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        match name.as_str() {
            "file" => {
                if file.is_some() {
                    return Err(ApiError::bad_request(anyhow::anyhow!(
                        "multipart form contains more than one file field"
                    )));
                }
                file = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|error| ApiError::bad_request(error.into()))?
                        .to_vec(),
                );
            }
            "engine" => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| ApiError::bad_request(error.into()))?;
                engine = Engine::from_str(&value).map_err(ApiError::bad_request)?;
            }
            "page_range" if allow_page_range => {
                let value = field
                    .text()
                    .await
                    .map_err(|error| ApiError::bad_request(error.into()))?;
                if !value.trim().is_empty() {
                    page_range = Some(PageRange::from_str(&value).map_err(ApiError::bad_request)?);
                }
            }
            _ => {}
        }
    }

    let file = file.ok_or_else(|| ApiError::bad_request(anyhow::anyhow!("missing file field")))?;
    if file.is_empty() {
        return Err(ApiError::bad_request(anyhow::anyhow!(
            "uploaded file is empty"
        )));
    }

    Ok(Upload {
        file,
        engine,
        page_range,
    })
}

struct ApiError {
    status: StatusCode,
    error: anyhow::Error,
}

impl ApiError {
    fn bad_request(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error,
        }
    }

    fn upstream(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            error,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::warn!(status = %self.status, error = %self.error, "request failed");
        (
            self.status,
            Json(ErrorResponse {
                error: format!("{:#}", self.error),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use serde_yaml::Value;
    use tower::ServiceExt;

    use super::{response_engine, router};
    use crate::{config::Config, models::Engine};

    #[test]
    fn response_engine_marks_mixed_page_engines() {
        assert_eq!(
            response_engine(&[Engine::Paddle, Engine::Glm], Engine::Paddle),
            "mixed"
        );
        assert_eq!(
            response_engine(&[Engine::Glm, Engine::Glm], Engine::Paddle),
            "glm"
        );
    }

    #[tokio::test]
    async fn versioned_routes_are_registered_without_removing_legacy_routes() {
        let app = router(Config::default()).expect("router should initialize");

        for path in ["/ocr/image", "/ocr/pdf", "/v1/ocr/image", "/v1/ocr/pdf"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .expect("request should build"),
                )
                .await
                .expect("router should respond");

            assert_eq!(
                response.status(),
                axum::http::StatusCode::METHOD_NOT_ALLOWED,
                "POST route {path} should exist"
            );
        }

        for path in ["/ocr/health", "/v1/ocr/health"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .body(Body::empty())
                        .expect("request should build"),
                )
                .await
                .expect("router should respond");

            assert_eq!(
                response.status(),
                axum::http::StatusCode::METHOD_NOT_ALLOWED,
                "GET route {path} should exist"
            );
        }
    }

    #[tokio::test]
    async fn openapi_contract_is_served_and_defines_public_routes() {
        let app = router(Config::default()).expect("router should initialize");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/openapi.yaml")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let contract =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/openapi.yaml"))
                .expect("openapi.yaml should exist");
        let document: Value =
            serde_yaml::from_str(&contract).expect("OpenAPI should be valid YAML");
        let paths = document["paths"]
            .as_mapping()
            .expect("OpenAPI paths should be a mapping");

        for path in ["/v1/ocr/health", "/v1/ocr/image", "/v1/ocr/pdf"] {
            assert!(
                paths.contains_key(Value::String(path.to_owned())),
                "OpenAPI should document {path}"
            );
        }
    }
}
