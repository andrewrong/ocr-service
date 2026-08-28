# Unified Local Inference Backend Research

Date: 2026-08-26

## Goal

Support Ollama, LM Studio, and llama.cpp without changing the OCR service's public HTTP routes or
making callers understand provider-specific request formats.

## Protocol overlap

All three local runtimes expose the OpenAI Chat Completions shape needed by this project:

- [Ollama OpenAI compatibility](https://docs.ollama.com/api/openai-compatibility) documents
  `/v1/chat/completions`, `/v1/models`, and a vision request using `text` plus an `image_url` data
  URL.
- [LM Studio OpenAI compatibility](https://lmstudio.ai/docs/developer/openai-compat) documents
  `/v1/chat/completions`, `/v1/models`, and text-and-image requests against a configurable base
  URL.
- [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
  documents OpenAI-compatible Chat Completions, model listing, and multimodal `image_url` input.

This common protocol covers the only provider operations the OCR service needs: submit one image
with a transcription prompt, and list visible models for readiness reporting.

## Rust library candidates

### Existing `reqwest` + local protocol types

The current implementation already uses `reqwest` and `serde`. Replacing the Ollama-specific wire
types with the small OpenAI-compatible request and response subset adds no runtime dependency and
keeps timeout, fallback, concurrency, and error behavior local to the OCR module.

### `async-openai`

[`async-openai`](https://github.com/64bit/async-openai) supports custom base URLs, custom request
configuration, Chat Completions, and OpenAI-compatible providers. It is a good general OpenAI
client, but this project would use only a small fraction of its interface and would still need
provider-specific model names, health semantics, fallback, and concurrency logic.

### `rust-genai`

[`rust-genai`](https://github.com/jeremychone/rust-genai) provides one interface across many hosted
and local providers, including a native Ollama adapter, custom endpoints, model listing, and image
analysis for selected adapters. Its provider resolution and broad normalization are useful for a
general AI application, but are wider than this OCR service's two-operation interface. LM Studio
would still be represented as a custom OpenAI-compatible endpoint.

### `rig`

[`rig`](https://github.com/0xPlaygrounds/rig) includes Ollama and shared OpenAI-compatible provider
infrastructure. Its agent, completion-model, tool, embedding, and provider abstractions solve a
larger problem than direct OCR inference and would add an unnecessary public dependency seam.

## Decision

Use the existing HTTP stack and introduce a single internal OpenAI-compatible adapter configured by:

- backend kind: `ollama`, `lmstudio`, or `llamacpp`;
- server base URL;
- optional bearer token;
- the existing logical Paddle, GLM, and Qwen model mappings.

The module interface remains `OcrEngine::ocr_image`, `OcrEngine::health`, and `model_name`. Provider
wire formats, authorization, image data URLs, response parsing, and model-list parsing stay inside
the implementation. This is a deep module: external OCR callers and SDKs keep one small interface
while backend variation remains local.

## Compatibility strategy

- Default to Ollama and retain `OCR_OLLAMA_URL` as a legacy configuration fallback.
- Add backend-neutral health fields while retaining the existing `ollama` boolean as a deprecated
  readiness alias for older SDK releases.
- Keep `/ocr/*` and `/v1/ocr/*` unchanged.
- Continue using the existing per-model timeouts, fallback order, and request semaphore.

## Model risk

Protocol compatibility does not guarantee model compatibility. LM Studio officially publishes a
[Qwen3-VL model with vision input](https://lmstudio.ai/models/qwen/qwen3-vl-8b), but the exact
PaddleOCR-VL and GLM OCR builds currently used by Ollama must be tested or replaced with LM
Studio-compatible model identifiers. The backend migration therefore requires a live image and PDF
regression for every configured model before Ollama can be removed.
