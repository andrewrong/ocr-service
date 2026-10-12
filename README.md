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

### Deploy a commit and roll back

`scripts/deploy.sh` deploys a commit of this repository, or rolls back to the one before, and
appends one line per attempt to a record:

```bash
OCR_DEPLOY_SERVICE_DIR=<dir> OCR_DEPLOY_RECORD=<file> scripts/deploy.sh [<commit>]
OCR_DEPLOY_SERVICE_DIR=<dir> OCR_DEPLOY_RECORD=<file> scripts/deploy.sh --rollback
```

- `OCR_DEPLOY_SERVICE_DIR` (required): the directory that holds the Compose file the service runs from.
- `OCR_DEPLOY_RECORD` (required): the file the record lines are appended to; created when missing.
- `OCR_DEPLOY_HEALTH_TIMEOUT` (optional, default `180`): seconds to wait for the health check.

A missing variable or wrong arguments print the usage, write no line and exit with 2.

A deploy fetches `origin`, resolves `<commit>` (default `origin/master`) to 40 characters, tags the
image running now as `ocr-service:<its commit>`, builds the commit as `git archive` gives it (not
the working tree) with the label `org.opencontainers.image.revision` and the tag
`ocr-service:<commit>`, points `ocr-service:local` at it, runs `docker compose up -d --no-build`
in the service directory and waits for the container to be `healthy`. A rollback points
`ocr-service:local` at the image of the `from` commit of the last `deploy` line with result `ok`,
then starts and waits the same way; nothing is built. The script never rolls back by itself and
removes no image.

The script also keeps the `compose.yaml` of the service directory at the one of the deployed
commit. After the build succeeded, when the two differ or there is none, the file there is saved
beside it as `compose.yaml.<commit running before>` (replacing an older copy of that name) and the
`compose.yaml` of the commit is written in its place; when they are equal nothing is written. A
rollback copies `compose.yaml.<target>` over `compose.yaml` when that file exists, after saving the
current one as `compose.yaml.<commit running before>`. In the service directory the script writes
only `compose.yaml` and these copies; it never reads, writes, moves or removes a `.env` file.

Each attempt past the usage check appends one line of five fields separated by a space:

```
<time> <deploy|rollback> <from> <to> <result>
```

`<time>` is RFC 3339 with an offset (`2026-10-12T18:30:00+08:00`); `<from>` is the commit running
before and `<to>` the commit asked for, 7 to 40 lowercase hexadecimal characters, `0000000` when
not known. `<result>` is one of `ok`, `fetch-failed`, `unknown-commit`, `build-failed`,
`up-failed`, `unhealthy`, `no-rollback-target`, `image-missing`. The exit code is 0 only for `ok`,
otherwise 1.

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
