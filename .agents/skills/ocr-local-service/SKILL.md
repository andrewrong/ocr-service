---
name: ocr-local-service
description: OCR image and PDF files through the Docker-hosted OCR HTTP service and return Markdown. Use to extract text or tables, convert scans to Markdown, check service health, or select Paddle, GLM, Qwen, or explicitly requested Jina cloud OCR.
---

# Local OCR Service

Use `scripts/ocr.sh` relative to this file. It depends on `curl` and `jq`; override the default endpoint with `OCR_SERVICE_URL`.

## Workflow

1. Resolve the input to a readable local file path and determine whether it is an image or PDF.
2. Run `bash scripts/ocr.sh health` once per turn. Inspect the requested engine's availability and the independent local/cloud health; surface degraded details.
3. Run the matching command. Use `auto` unless the user requests an engine. Add `--pages` only for a requested PDF page or inclusive range.
4. Return the Markdown written to stdout faithfully. Summarize or transform it only when the user asks.

```bash
bash scripts/ocr.sh image /absolute/path/to/image.png
bash scripts/ocr.sh image /absolute/path/to/image.png --engine glm
bash scripts/ocr.sh pdf /absolute/path/to/file.pdf --pages 2-7
bash scripts/ocr.sh pdf /absolute/path/to/file.pdf --engine qwen --json
```

`--json` returns the complete response with engine, page count, and duration. Set `OCR_TIMEOUT_SECS` for unusually large PDFs.

## Jina cloud OCR

Use `--engine jina` only when the user explicitly requests Jina and accepts uploading document pages to the hosted API. `auto` always stays local. The server owns the private `OCR_JINA_API_KEY`; clients never receive or forward it.

```bash
bash scripts/ocr.sh image /absolute/path/to/image.png --engine jina --json
bash scripts/ocr.sh pdf /absolute/path/to/file.pdf --engine jina --pages 1-10 --json
```

The server retries transient cloud errors once within the page budget, then falls back to GLM, Paddle, and Qwen. Report the returned actual engine (or `mixed`), not the requested engine. A missing key is a configuration error. Cloud metadata reachability does not validate the key, balance, or inference capacity. After a cloud failure, surface the error rather than automatically replaying the upload; uncertain outcomes may already have incurred usage.

## Failure handling

- A connection failure means the HTTP service is unavailable. Report the endpoint and start the Compose service only when managing this repository is in scope.
- An HTTP error includes the service's error message on stderr. Preserve that message when reporting the failure.
- A missing dependency is a local setup error; name the missing executable.
