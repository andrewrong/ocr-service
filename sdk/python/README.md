# OCR Service Python Client

Typed asynchronous client for the local OCR service. It exposes one `recognize()` method for images
and PDFs and a `health()` method for readiness checks.

```python
from ocr_service_client import OcrClient

async with OcrClient("http://127.0.0.1:18100") as client:
    result = await client.recognize("scan.pdf", pages="1-5")
    print(result.markdown)
```

See `../../docs/integration.md` for the complete HTTP contract and deployment addresses.

Use `recognize("scan.pdf", engine="jina", pages="1-10")` to explicitly upload pages to
Jina through the OCR service. Configure the private key on the server only. `auto` remains local.
Inspect the actual returned engine because cloud failures can fall back locally. Health exposes
optional `health.jina.configured`/`reachable`; `health.ready` covers any available provider.
Cloud metadata does not validate credentials or capacity. Avoid automatic upload replay after
uncertain network errors because it can incur duplicate usage. Existing tagged versions do not
include Jina yet; use this checkout until a new version is published.
