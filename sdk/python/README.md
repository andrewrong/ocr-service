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
