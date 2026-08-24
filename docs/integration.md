# OCR Service Integration

The OCR service accepts an uploaded image or PDF and returns Markdown. Callers should use the
versioned `/v1/ocr/*` routes. Existing `/ocr/*` routes remain available as compatibility aliases.

## Addresses

| Caller | Base URL |
|---|---|
| Process on the macOS host | `http://127.0.0.1:18100` |
| Process in an OrbStack/Docker container | `http://host.docker.internal:18100` |

Configure the address instead of hard-coding it:

```bash
OCR_SERVICE_URL=http://host.docker.internal:18100
```

The deployment binds to the local host by default and does not implement authentication. Do not
publish it to a LAN or the internet without an authenticated reverse proxy and TLS.

## HTTP contract

The machine-readable OpenAPI 3.1 contract is available in the repository as `openapi.yaml` and
from a running service at `GET /openapi.yaml`.

### Image

```bash
curl --fail-with-body \
  --form "file=@receipt.png" \
  --form "engine=auto" \
  "$OCR_SERVICE_URL/v1/ocr/image"
```

### PDF

```bash
curl --fail-with-body \
  --form "file=@document.pdf" \
  --form "engine=auto" \
  --form "page_range=2-7" \
  --max-time 900 \
  "$OCR_SERVICE_URL/v1/ocr/pdf"
```

`engine` accepts `auto`, `paddle`, `glm`, or `qwen`. Prefer `auto`: the server owns model selection,
per-model timeouts, and timeout fallback. `page_range` is inclusive, one-based, and valid only for
PDFs.

A successful response has this shape:

```json
{
  "markdown": "# Recognized content",
  "engine": "paddle",
  "pages": 1,
  "duration_ms": 1234
}
```

For a PDF, `engine` is `mixed` when different pages complete with different fallback engines.
Invalid input returns HTTP 400, oversized requests return 413, and OCR/model failures return 502.
Clients should allow up to 15 minutes for large PDFs.

## Python client

Install the local package with uv:

```bash
uv add --editable /path/to/ocr-service/sdk/python
```

Use one high-level method for both images and PDFs:

```python
import asyncio

from ocr_service_client import OcrClient


async def main() -> None:
    async with OcrClient("http://host.docker.internal:18100") as client:
        result = await client.recognize("/data/document.pdf", pages="1-10")
        print(result.markdown)


asyncio.run(main())
```

The client detects PDFs, selects the correct HTTP route, constructs the multipart request, validates
the engine/page range, and converts HTTP or protocol failures into `OcrServiceError`. During a
rolling deployment it retries the matching legacy `/ocr/*` route only when a `/v1/ocr/*` route
returns HTTP 404.

## Go client

The dependency-free Go client lives in `sdk/go`. Install the tagged module directly from GitHub:

```bash
go get github.com/andrewrong/ocr-service/sdk/go@v0.1.0
```

For local SDK development, add a temporary replacement with
`go mod edit -replace=github.com/andrewrong/ocr-service/sdk/go=/path/to/ocr-service/sdk/go`.

```go
client, err := ocrclient.NewClient("http://host.docker.internal:18100")
if err != nil {
    return err
}
result, err := client.Recognize(ctx, "/data/document.pdf", &ocrclient.RecognizeOptions{
    Engine: ocrclient.EngineAuto,
    Pages:  "1-10",
})
```

The Go adapter streams multipart data instead of buffering the entire document, accepts an injected
`http.Client`, returns typed `ServiceError` values for HTTP failures, and has the same versioned-route
fallback behavior as the Python adapter.
