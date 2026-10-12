# Instructions for agents working on this repository

`CLAUDE.md` describes the service: its architecture, the models, the API and how it runs.
This file says what a change has to do. Where the two disagree on a project fact, fix the one
that is wrong in the same pull request.

## Build and test

Before you push, all three pass from the repository root:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

A change under `sdk/go` also passes `go vet ./... && go test ./...` there, and a change under
`sdk/python` passes `uv run --locked --extra test pytest` there. The workflow in
`.github/workflows/ci.yml` runs the same commands on every pull request.

## Tests

- A test talks only to servers it starts itself on `127.0.0.1` (see `tests/jina.rs`). No test
  calls a hosted OCR API, a model runtime or a running instance of this service.
- The live test of the Go SDK runs only when `OCR_LIVE_TEST_URL` is set. Do not set it.
- Every behaviour change comes with a test.

## What a change does not touch

- `.env` files: never read, print or commit one. `.env.example` holds names and placeholder
  values only.
- An installed instance of the service (by default on `127.0.0.1:18100`): do not start it,
  stop it or send it a request, and do not bind that port.
- The deployment files (`compose.yaml`, `Dockerfile`, `.env.example`, `scripts/`) and the skill
  that callers install (`.agents/`) change only when the issue asks for it.
- Cloud use is opt-in: a request that does not name the `jina` engine never uploads a page.
  Do not change that, or how a key is read, stored or logged, unless the issue asks for it.

## The contract

`openapi.yaml` is the public contract, and the service serves it. A change to a route, or to a
field of a request or a response, changes `openapi.yaml`, `docs/integration.md` and the SDKs
under `sdk/` that expose it, in the same pull request.

## Conventions

- English for code, comments, commit messages and pull request text. Commit subjects follow the
  history: `feat: ...`, `fix: ...`, `chore: ...`.
- Errors are `anyhow::Result`, logs go through `tracing`, the async runtime is `tokio`.
- Do not add a dependency the issue does not mention.
- The default branch is `master`. Never push to it; work reaches it through a pull request.

## The repository is public

No secret, token, personal data, path of a private machine or real document in code, fixtures,
commit messages, issues or pull request text.
