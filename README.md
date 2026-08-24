# OCR Service

Local HTTP OCR service backed by Ollama. Docker contains the Rust API and Poppler; Ollama and its models stay on the macOS host for Metal acceleration.

## Deploy

Prerequisites: OrbStack, Ollama, PaddleOCR-VL 1.6, GLM OCR, and Qwen3-VL 8B.

Allow the container to reach Ollama, then restart the Ollama application:

```bash
launchctl setenv OLLAMA_HOST 0.0.0.0:11434
```

Binding Ollama to all interfaces can expose it to the local network. Keep the macOS firewall enabled or restrict access with local firewall rules.

Start and verify the service:

```bash
cp .env.example .env
docker compose up -d --build
docker compose ps
curl -fsS http://127.0.0.1:18100/ocr/health | jq
```

Stop it with `docker compose down`. The API port binds to `127.0.0.1` by default.

## Integrate another service

Use the versioned HTTP routes for new callers:

```bash
curl --fail-with-body \
  --form "file=@document.pdf" \
  --form "engine=auto" \
  --form "page_range=1-10" \
  http://127.0.0.1:18100/v1/ocr/pdf
```

The OpenAPI 3.1 contract is stored in [`openapi.yaml`](openapi.yaml) and served at
`GET /openapi.yaml`. Detailed curl, Docker, timeout, response, and security guidance lives in
[`docs/integration.md`](docs/integration.md). Python callers can install the typed asynchronous
client from [`sdk/python`](sdk/python) and use one `OcrClient.recognize()` method for images and
PDFs. Go callers can use the dependency-free streaming client in [`sdk/go`](sdk/go). Existing
`/ocr/*` routes remain available for compatibility.

## Agent Skill

The project Skill lives at `.agents/skills/ocr-local-service`. It uploads host files over HTTP, so the service container needs no filesystem mounts.

```bash
bash .agents/skills/ocr-local-service/scripts/ocr.sh image /path/to/image.png
bash .agents/skills/ocr-local-service/scripts/ocr.sh pdf /path/to/file.pdf --pages 2-7
```

To install it personally, copy the entire directory:

```bash
cp -R .agents/skills/ocr-local-service ~/.codex/skills/ocr-local-service
```

Set `OCR_SERVICE_URL` when the service is not at `http://127.0.0.1:18100`.

## Model timeouts and fallback

Every rendered PDF page and uploaded image has a model-specific timeout. The defaults are 30
seconds for Paddle, 60 seconds for GLM, and 120 seconds for Qwen. Override them with
`OCR_PADDLE_TIMEOUT_SECS`, `OCR_GLM_TIMEOUT_SECS`, and `OCR_QWEN_TIMEOUT_SECS`.

`auto` and `paddle` try Paddle, GLM, then Qwen. `glm` tries GLM, Paddle, then Qwen. `qwen` tries
Qwen, GLM, then Paddle. Fallback happens only after a timeout; other upstream errors are returned
immediately. A PDF response reports `engine: "mixed"` when different pages use different models.
Ollama model calls default to one active request (`OCR_MAX_CONCURRENT_MODEL_REQUESTS=1`) so queue
time is not mistaken for model execution time; PDF page rendering and preparation remain concurrent.
