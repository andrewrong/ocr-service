# OCR Service

## Scope

A Rust HTTP service that turns PDFs and images into Markdown. It calls local vision models
(PaddleOCR-VL, GLM-OCR, Qwen3-VL) through one OpenAI-compatible adapter that talks to Ollama,
LM Studio or llama.cpp, and optionally the hosted Jina OCR API. It normally runs under Docker
Compose and reaches the inference runtime on the host.

Read [README.md](README.md) and [docs/integration.md](docs/integration.md) before a change
that affects the HTTP contract, configuration, deployment or the cloud engine.
[openapi.yaml](openapi.yaml) is the contract; [CLAUDE.md](CLAUDE.md) is the short overview.

## Project map

- `src/main.rs` parses the command line and starts the server; `src/lib.rs` exposes the modules.
- `src/config.rs` reads every setting from the environment and validates it.
- `src/models.rs` holds the public request and response types, including `Engine`.
- `src/http/routes.rs` owns the routes, multipart parsing and the mapping of errors to statuses.
- `src/ocr/engine.rs` owns engine selection, the fallback order, timeouts and the health snapshot.
- `src/ocr/backend.rs` is the OpenAI-compatible client for the local runtimes.
- `src/ocr/jina.rs` is the hosted Jina client.
- `src/ocr/pdf.rs` splits a PDF into pages with `pdftoppm`; `src/ocr/merger.rs` joins page results.
- `tests/` holds integration tests that run against servers they start themselves.
- `sdk/python` and `sdk/go` are the client SDKs; `.agents/skills/ocr-local-service` is the agent
  skill, a `curl` and `jq` client.
- `Dockerfile` and `compose.yaml` build and run the service; `.env.example` lists the settings.

## Rules

- **The contract moves as one.** A change to a request, a response or a setting changes, in the
  same pull request, `openapi.yaml`, both SDKs, the skill, `.env.example`, `compose.yaml` and the
  documents that describe it.
- **Local stays local.** A request that does not name a cloud engine sends no page to a hosted
  API unless the operator has turned that on with a setting that is off by default. A change to
  when a page may leave the machine says so in the description of its pull request.
- **Credentials.** A key is read from the environment only. It is never logged, never returned
  in a response and never written to a file of the repository. A change to how a credential is
  read, stored, logged or sent says so in the description of its pull request.
- **Tests are closed.** A test or a fixture talks only to servers it starts itself on
  `127.0.0.1`, on a port the system picks. None sends a request to a hosted OCR API or to a
  model runtime, downloads a model, or needs Docker.
- **The installed service is not part of the work.** Nothing starts, stops, rebuilds or sends a
  request to a running instance of the service, binds its port, or reads or writes its directory
  or any `.env` file. Deployment is done by the owner.
- **Defaults are stable.** The default engine, the fallback order of the local engines and the
  per-engine timeouts change only when the issue asks for it.

## Code

- Errors are `anyhow::Result`; logs go through `tracing`; the async runtime is `tokio`.
- Match the code around a change: its naming, its comment density and its idiom.
- A change of behaviour comes with a test that fails without it.

## Checks

All three pass before a pull request is opened:

```
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```
