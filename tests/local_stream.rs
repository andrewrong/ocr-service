use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request as ExtractRequest, State},
    http::{HeaderValue, Request, StatusCode, header::CONTENT_TYPE},
    response::Response,
    routing::post,
};
use ocr_service::config::Config;
use serde_json::{Value, json};
use tower::ServiceExt;

/// What the local runtime fixture saw.
#[derive(Default)]
struct Seen {
    bodies: Mutex<Vec<Value>>,
    calls: AtomicUsize,
    sent_chars: AtomicUsize,
    closed: AtomicBool,
}

/// Counts what a body stream sent and notes when the client dropped it.
struct Watch(Arc<Seen>);

impl Drop for Watch {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
    }
}

fn event(content: &str, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices":[{"delta":{"content":content},"finish_reason":finish}]})
    )
}

fn sse(body: Body) -> Response {
    let mut response = Response::new(body);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    response
}

/// A stream that sends `head` once, then `tail` again and again, one piece every millisecond,
/// and never ends.
fn endless(seen: Arc<Seen>, head: Vec<String>, tail: Vec<String>) -> Response {
    let watch = Watch(seen.clone());
    let stream = futures::stream::unfold(
        (watch, head.into_iter(), tail, 0usize),
        |(watch, mut head, tail, index)| async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            let (piece, index) = match head.next() {
                Some(piece) => (piece, index),
                None => (tail[index % tail.len()].clone(), index + 1),
            };
            watch.0.sent_chars.fetch_add(piece.len(), Ordering::SeqCst);
            Some((
                Ok::<_, std::io::Error>(event(&piece, None)),
                (watch, head, tail, index),
            ))
        },
    );
    sse(Body::from_stream(stream))
}

/// A stream that sends `pieces`, a final event with `finish`, and `[DONE]`.
fn finished(pieces: &[&str], finish: &str) -> Response {
    let mut body: String = pieces.iter().map(|piece| event(piece, None)).collect();
    body.push_str(&event("", Some(finish)));
    body.push_str("data: [DONE]\n\n");
    sse(Body::from(body))
}

async fn runtime(
    handler: impl Fn(Arc<Seen>, Value) -> Response + Clone + Send + Sync + 'static,
) -> (String, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let router = Router::new()
        .route(
            "/v1/chat/completions",
            post(
                move |State(seen): State<Arc<Seen>>, request: ExtractRequest| {
                    let handler = handler.clone();
                    async move {
                        let body = to_bytes(request.into_body(), usize::MAX).await.unwrap();
                        let body: Value = serde_json::from_slice(&body).unwrap();
                        seen.calls.fetch_add(1, Ordering::SeqCst);
                        seen.bodies.lock().unwrap().push(body.clone());
                        handler(seen, body)
                    }
                },
            ),
        )
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, seen)
}

fn config(url: String) -> Config {
    Config {
        inference_url: url,
        paddle_model: "paddle-test".into(),
        glm_model: "glm-test".into(),
        qwen_model: "qwen-test".into(),
        local_max_output_tokens: 1234,
        ..Config::default()
    }
}

async fn post_image(config: Config, engine: &str) -> (StatusCode, Value) {
    let app = ocr_service::http::router(config).unwrap();
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

async fn closed(seen: &Seen) -> bool {
    for _ in 0..200 {
        if seen.closed.load(Ordering::SeqCst) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

const LINES: [&str; 3] = [
    "First line of a document with some words in it, and more words.\n",
    "Second line, which differs from the first one a lot, and goes on.\n",
    "Third and last line of the repeated block, long enough to count for sure.\n",
];

#[tokio::test]
async fn every_local_request_streams_and_carries_the_configured_cap() {
    let (url, seen) = runtime(|_, _| finished(&["hello ", "world"], "stop")).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["markdown"], "hello world");
    let bodies = seen.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["stream"], true);
    assert_eq!(bodies[0]["max_tokens"], 1234);
}

#[tokio::test]
async fn a_loop_of_lines_is_cut_and_the_stream_closed_early() {
    let block: usize = LINES.iter().map(|line| line.len()).sum();
    assert!(block >= 200);
    let head: Vec<String> = LINES.iter().map(|line| line.to_string()).collect();
    let tail = head.clone();
    let (url, seen) = runtime(move |seen, _| endless(seen, head.clone(), tail.clone())).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "glm");
    assert_eq!(body["markdown"], LINES.concat().trim());
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
    assert!(closed(&seen).await, "the stream was not closed");
    assert!(seen.sent_chars.load(Ordering::SeqCst) < block + 3000);
}

#[tokio::test]
async fn a_loop_inside_one_line_is_cut_and_the_stream_closed_early() {
    let phrase = "and then it said the same thing again, ";
    assert!((35..=45).contains(&phrase.len()));
    let intro = "Intro text before the loop starts: ".to_owned();
    let head = vec![intro.clone()];
    let tail = vec![phrase.to_owned()];
    let (url, seen) = runtime(move |seen, _| endless(seen, head.clone(), tail.clone())).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "glm");
    let text = body["markdown"].as_str().unwrap();
    assert_eq!(text, intro.trim());
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
    assert!(closed(&seen).await, "the stream was not closed");
    assert!(seen.sent_chars.load(Ordering::SeqCst) < intro.len() + 3000);
}

#[tokio::test]
async fn a_short_line_repeated_three_times_is_not_a_loop() {
    let text = "Heading\nrow | 1\nrow | 1\nrow | 1\nother text after the rows\n";
    let (url, seen) = runtime(move |_, _| finished(&[text], "stop")).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["markdown"], text.trim());
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_truncated_text_without_repetition_fails_the_attempt_and_the_next_engine_runs() {
    let (url, seen) = runtime(|_, body| {
        if body["model"] == "glm-test" {
            finished(&["an unfinished text"], "length")
        } else {
            finished(&["complete text"], "stop")
        }
    })
    .await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "paddle");
    assert_eq!(body["markdown"], "complete text");
    assert_eq!(seen.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_truncated_tail_with_an_incomplete_last_copy_is_cut_without_fallback() {
    let block = LINES.concat();
    let intro = "Intro line.\n";
    let text = format!("{intro}{block}{block}{block}{}", &LINES[0][..20]);
    let (url, seen) = runtime(move |_, _| finished(&[&text], "length")).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "glm");
    assert_eq!(body["markdown"], format!("{intro}{block}").trim());
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn truncated_crlf_unicode_repetition_is_cut_without_panicking() {
    let (url, seen) = runtime(|_, _| finished(&["Intro\r\n界\r\n界\r\n界"], "length")).await;

    let (status, body) = post_image(config(url), "glm").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["engine"], "glm");
    assert_eq!(body["markdown"], "Intro\n界");
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
}
