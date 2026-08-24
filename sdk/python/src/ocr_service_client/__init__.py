"""Public interface for the OCR service Python client."""

from ocr_service_client.client import OcrClient
from ocr_service_client.exceptions import OcrServiceError
from ocr_service_client.models import HealthResult, ModelStatus, OcrResult

__all__ = [
    "HealthResult",
    "ModelStatus",
    "OcrClient",
    "OcrResult",
    "OcrServiceError",
]
