---
name: ocr-local-service
description: OCR local image and PDF files through the Docker-hosted OCR HTTP service and return Markdown. Use when the user asks to recognize, transcribe, extract text or tables, or convert a scanned document to Markdown; also use to check the local OCR service or compare Paddle, GLM, and Qwen engines.
---

# Local OCR Service

Use `scripts/ocr.sh` relative to this file. It depends on `curl` and `jq`; override the default endpoint with `OCR_SERVICE_URL`.

## Workflow

1. Resolve the input to a readable local file path and determine whether it is an image or PDF.
2. Run `bash scripts/ocr.sh health` once per turn. Continue when Ollama is reachable; surface the returned health details when it is degraded.
3. Run the matching command. Use `auto` unless the user requests an engine. Add `--pages` only for a requested PDF page or inclusive range.
4. Return the Markdown written to stdout faithfully. Summarize or transform it only when the user asks.

```bash
bash scripts/ocr.sh image /absolute/path/to/image.png
bash scripts/ocr.sh image /absolute/path/to/image.png --engine glm
bash scripts/ocr.sh pdf /absolute/path/to/file.pdf --pages 2-7
bash scripts/ocr.sh pdf /absolute/path/to/file.pdf --engine qwen --json
```

`--json` returns the complete response with engine, page count, and duration. Set `OCR_TIMEOUT_SECS` for unusually large PDFs.

## Failure handling

- A connection failure means the HTTP service is unavailable. Report the endpoint and start the Compose service only when managing this repository is in scope.
- An HTTP error includes the service's error message on stderr. Preserve that message when reporting the failure.
- A missing dependency is a local setup error; name the missing executable.
