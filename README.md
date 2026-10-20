# OCR Service

Local HTTP OCR service backed by an OpenAI-compatible local model runtime. Ollama is the default;
LM Studio and llama.cpp are also supported. Docker contains the Rust HTTP layer and Poppler while
models stay on the macOS host for Metal acceleration. Explicit `jina` requests can optionally
use Jina's hosted OCR API instead; local defaults do not upload document pages unless the operator
sets `OCR_JINA_FALLBACK_ON_LOCAL_FAILURE=true`, which lets a local strategy make one last Jina
attempt after every local engine failed.

## Deploy

Prerequisites: OrbStack and a supported local vision runtime, or a configured Jina key for explicit
cloud requests. Local fallback requires at least one configured local vision model.

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

Then set these in `ocr-service.env` (see below):

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

### Deployment by crew

crew deploys this service (docker way): after a merge it builds the commit with
`docker compose -p ocr-service --project-directory <checkout> build`, starts it with `up -d` and
rolls back to the image of the commit before when the deploy fails. `compose.yaml` takes exactly
three variables from crew: `CREW_SERVICE_DIR`, `CREW_SECRETS` (the directory that holds
`ocr-service.env`) and `CREW_IMAGE` (`ocr-service:<7-hex commit>`). All are required; there is no
fallback.

The settings are the keys of `ocr-service.env`, which `.env.example` lists with their defaults. Copy
it to `ocr-service.env` in the `CREW_SECRETS` directory and set the private values there (the
file is read by compose as an `env_file`, so it overrides nothing and is never committed).
`OCR_INFERENCE_URL` must point at the host from inside the container, for example
`http://host.docker.internal:11434`; the code default `http://127.0.0.1:11434` is the container
itself. The API port is fixed to `127.0.0.1:18100`.

Without crew, set the three variables yourself and run compose from the repository root:

```bash
CREW_SERVICE_DIR=$PWD CREW_SECRETS=<dir-with-ocr-service.env> CREW_IMAGE=ocr-service:local \
  docker compose up -d --build
docker compose ps
curl -fsS http://127.0.0.1:18100/ocr/health | jq
```

Stop it with `docker compose down` (with the same variables set).

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

## Optional Jina cloud OCR

Set `OCR_JINA_API_KEY` only in the private deployment `.env`, then rebuild the OCR container.
`OCR_JINA_BASE_URL` defaults to `https://api.jina.ai`; the per-page `OCR_JINA_TIMEOUT_SECS`
defaults to 120 seconds, including one retry and backoff. Explicit `engine=jina` uploads image
pages to Jina and can incur charges; `auto` and the local strategies stay local unless
`OCR_JINA_FALLBACK_ON_LOCAL_FAILURE=true` (default `false`, needs a key), when a page is also
uploaded as a last attempt after every local engine failed. Error responses carry
`cloud_attempted`, telling whether Jina may have been called.

```bash
curl --fail-with-body -F 'file=@document.pdf' -F 'engine=jina' -F 'page_range=1-10' \
  --max-time 900 http://127.0.0.1:18100/v1/ocr/pdf
```

Transient cloud errors are retried once, then GLM, Paddle and Qwen are attempted locally.
Read the actual returned `engine`, which can be `jina`, a local engine, or `mixed` for PDFs.
Missing credentials return HTTP 400. See [integration guidance](docs/integration.md#jina-cloud-ocr)
for privacy, retry, health and client examples. This is hosted inference, not a local Jina model.

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
