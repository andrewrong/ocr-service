# Jina OCR integration findings

Verified 2026-09-18 against first-party sources. Chosen route: hosted API only.

## Hosted contract

- Endpoint: `https://api.jina.ai/v1/chat/completions`, Bearer key, model `jina-ocr-v1`.
- A message carries text plus `image_url`; local files can use image data URLs. Use the default
  Markdown prompt rather than the benchmark prompt, which explicitly requests HTML tables and
  header/footer omission. Cold starts can return 503; the model card recommends retrying after
  30–60 seconds. [Official model card](https://huggingface.co/jinaai/jina-ocr-v1#hosted-api)
- The hosted schema uses `max_completion_tokens`; unsupported fields are accepted but ignored.
  Non-streaming output uses the OpenAI chat-completion response. 429 and transient 5xx responses
  require bounded backoff; authentication/balance errors are not transient.
  [Official OpenAPI](https://api.jina.ai/openapi.json)
- Public model metadata is available at `/v1/models/jina-ocr-v1`; its canonical returned ID is
  `jina-ai/jina-ocr-v1`. This endpoint confirms metadata reachability, not credential validity,
  account balance or inference readiness. [Model metadata](https://api.jina.ai/v1/models/jina-ocr-v1)

## Why not an immediate local deployment

The machine is an Apple M4 Pro with 64 GiB RAM. Official code supports Transformers CPU
inference, so local execution is not inherently impossible. It defaults to CUDA if available,
otherwise CPU, and supplies no validated MPS path. Local speed was not measured.
[Official example](https://huggingface.co/jinaai/jina-ocr-v1/blob/main/example.py)

The Jina checkpoint uses `DeepseekOCRForCausalLM` with custom processor/model code. No official
GGUF or MLX artifact was found in the model repository. Supporting the DeepSeek OCR base
architecture does not establish compatibility with this specific checkpoint and its MTP weights.
[Configuration](https://huggingface.co/jinaai/jina-ocr-v1/blob/main/config.json),
[Published files](https://huggingface.co/jinaai/jina-ocr-v1/tree/main),
[Ollama base model](https://ollama.com/library/deepseek-ocr)

## Implementation decisions

Cloud has separate credentials and endpoint; local `auto` and fallback chains never reach cloud.
PDFs keep the existing local page renderer. Explicit Jina tries cloud under a 120-second page
budget, then local GLM/Paddle/Qwen. Telegram disables whole-update replay for Jina tasks because
uncertain network/delivery failures could otherwise repeat paid inference. Tests use isolated
HTTP fixtures, including real Poppler rendering; no model download or paid OCR was performed.
