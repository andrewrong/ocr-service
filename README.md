# OCR Service

Local HTTP OCR service backed by an OpenAI-compatible local model runtime. Ollama is the default;
LM Studio and llama.cpp are also supported. Docker contains the Rust HTTP layer and Poppler while
models stay on the macOS host for Metal acceleration.

## Deploy

Prerequisites: OrbStack and at least one supported local runtime with a vision model.

| Runtime | `OCR_INFERENCE_BACKEND` | Default host port |
|---|---|---|
| Ollama | `ollama` | `11434` |
| LM Studio | `lmstudio` | `1234` |
| llama.cpp | `llamacpp` | `8080` |

Allow the container to reach Ollama, then restart the Ollama application:

```bash
launchctl setenv OLLAMA_HOST 0.0.0.0:11434
```

For LM Studio, start its server on a container-accessible address:

```bash
lms server start --port 1234 --bind 0.0.0.0
```

Then configure `.env`:

```bash
OCR_INFERENCE_BACKEND=lmstudio
OCR_INFERENCE_URL=http://host.docker.internal:1234
OCR_INFERENCE_API_TOKEN=
OCR_PADDLE_MODEL=<primary-vision-model-id>
OCR_GLM_MODEL=<first-fallback-model-id>
OCR_QWEN_MODEL=<second-fallback-model-id>
```

Use `curl http://127.0.0.1:1234/v1/models | jq -r '.data[].id'` to obtain exact model IDs.
LM Studio can require an API token; set it in `OCR_INFERENCE_API_TOKEN`. Binding any runtime to all
interfaces can expose it to the local network, so use authentication and firewall restrictions.

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
Qwen, GLM, then Paddle. Timeout and upstream model errors both advance to the next configured model.
A PDF response reports `engine: "mixed"` when different pages use different models. Inference calls
default to one active request (`OCR_MAX_CONCURRENT_MODEL_REQUESTS=1`) so queue time is not mistaken
for model execution time; PDF page rendering and preparation remain concurrent. When multiple
logical slots use the same model ID, that model is attempted only once per page.
