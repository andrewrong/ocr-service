"""Public interface for the OCR service Python client."""

from ocr_service_client.client import OcrClient
from ocr_service_client.exceptions import OcrServiceError
from ocr_service_client.models import HealthResult, JinaHealth, ModelStatus, OcrResult

__all__ = [
    "HealthResult",
    "JinaHealth",
    "ModelStatus",
    "OcrClient",
    "OcrResult",
    "OcrServiceError",
]
