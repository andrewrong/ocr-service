"""Asynchronous adapter for the OCR service HTTP interface."""

from __future__ import annotations

import json
import mimetypes
import os
import re
from pathlib import Path
from types import TracebackType
from typing import Any, Literal, cast

import httpx

from ocr_service_client.exceptions import OcrServiceError
from ocr_service_client.models import HealthResult, ModelStatus, OcrResult

Engine = Literal["auto", "paddle", "glm", "qwen"]
_ENGINES = frozenset({"auto", "paddle", "glm", "qwen"})
_PAGE_RANGE_PATTERN = re.compile(r"^[1-9][0-9]*(?:-[1-9][0-9]*)?$")


class OcrClient:
    """Recognize local images and PDFs through the OCR HTTP service.

    The client owns model-independent concerns: file type detection, route selection,
    multipart encoding, option validation, response parsing, and error normalization.

    Args:
        base_url: OCR service base URL. Defaults to `OCR_SERVICE_URL`, then the
            local host deployment address.
        timeout_seconds: Overall request timeout. Large PDFs may require several minutes.
        http_client: Optional injected client. Injected clients remain owned by the caller.
    """

    def __init__(
        self,
        base_url: str | None = None,
        *,
        timeout_seconds: float = 900.0,
        http_client: httpx.AsyncClient | None = None,
    ) -> None:
        """Initialize the client and its HTTP adapter."""
        if timeout_seconds <= 0:
            raise ValueError("timeout_seconds must be greater than zero")

        resolved_url = base_url or os.environ.get("OCR_SERVICE_URL", "http://127.0.0.1:18100")
        if not resolved_url.strip():
            raise ValueError("base_url cannot be empty")

        self.base_url = resolved_url.rstrip("/")
        self._owns_client = http_client is None
        self._client = http_client or httpx.AsyncClient(
            timeout=httpx.Timeout(timeout_seconds, connect=5.0),
            headers={"User-Agent": "ocr-service-client/0.1.0"},
        )

    async def recognize(
        self,
        file: str | Path,
        *,
        engine: Engine | str = "auto",
        pages: str | None = None,
    ) -> OcrResult:
        """Recognize an image or PDF and return Markdown with metadata.

        Args:
            file: Readable local image or PDF path.
            engine: Requested engine. Prefer `auto` for server-managed fallback.
            pages: Optional inclusive PDF page or range, for example `2` or `2-7`.

        Returns:
            Parsed OCR result.

        Raises:
            ValueError: If local inputs or options are invalid.
            OcrServiceError: If transport, HTTP, or response parsing fails.
        """
        path = self._validate_file(file)
        engine_value = self._validate_engine(engine)
        is_pdf = self._is_pdf(path)
        page_range = self._validate_pages(pages, is_pdf=is_pdf)
        endpoint = "/v1/ocr/pdf" if is_pdf else "/v1/ocr/image"
        legacy_endpoint = "/ocr/pdf" if is_pdf else "/ocr/image"
        content_type = "application/pdf" if is_pdf else self._image_content_type(path)
        form = {"engine": engine_value}
        if page_range is not None:
            form["page_range"] = page_range

        response = await self._upload(
            path,
            content_type=content_type,
            form=form,
            endpoint=endpoint,
            legacy_endpoint=legacy_endpoint,
        )

        payload = self._response_payload(response)
        try:
            return OcrResult(
                markdown=str(payload["markdown"]),
                engine=str(payload["engine"]),
                pages=int(payload["pages"]),
                duration_ms=int(payload["duration_ms"]),
            )
        except (KeyError, TypeError, ValueError) as error:
            raise OcrServiceError(
                "OCR service returned an invalid success response",
                status_code=response.status_code,
            ) from error

    async def health(self) -> HealthResult:
        """Return inference backend and configured model availability.

        A structured degraded response (HTTP 503) is returned as a non-ready result so callers
        can inspect individual models. Other HTTP failures raise `OcrServiceError`.
        """
        response = await self._get_with_legacy_fallback(
            "/v1/ocr/health",
            "/ocr/health",
        )

        if response.status_code not in {200, 503}:
            self._raise_http_error(response)
        payload = self._json_object(response)
        try:
            models = tuple(
                ModelStatus(
                    engine=str(model["engine"]),
                    name=str(model["name"]),
                    available=bool(model["available"]),
                )
                for model in payload["models"]
            )
            return HealthResult(
                status=str(payload["status"]),
                backend=str(payload.get("backend", "ollama")),
                backend_ready=bool(payload.get("backend_ready", payload.get("ollama", False))),
                ollama=bool(payload.get("ollama", False)),
                models=models,
            )
        except (KeyError, TypeError, ValueError) as error:
            raise OcrServiceError(
                "OCR service returned an invalid health response",
                status_code=response.status_code,
            ) from error

    async def close(self) -> None:
        """Close the internally owned HTTP client."""
        if self._owns_client:
            await self._client.aclose()

    async def __aenter__(self) -> OcrClient:
        """Enter an asynchronous client context."""
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        """Close owned resources when leaving an asynchronous context."""
        await self.close()

    @staticmethod
    def _validate_file(file: str | Path) -> Path:
        path = Path(file).expanduser()
        if not path.is_file():
            raise ValueError(f"file is not readable: {path}")
        return path

    @staticmethod
    def _validate_engine(engine: Engine | str) -> str:
        value = str(engine).strip().lower()
        if value not in _ENGINES:
            raise ValueError("engine must be one of: auto, paddle, glm, qwen")
        return value

    @staticmethod
    def _validate_pages(pages: str | None, *, is_pdf: bool) -> str | None:
        if pages is None or not pages.strip():
            return None
        value = pages.strip()
        if not is_pdf:
            raise ValueError("pages is only valid for PDF files")
        if not _PAGE_RANGE_PATTERN.fullmatch(value):
            raise ValueError("pages must be a one-based page or inclusive range such as 2-7")
        if "-" in value:
            start, end = (int(part) for part in value.split("-", maxsplit=1))
            if start > end:
                raise ValueError("pages range start must be less than or equal to end")
        return value

    @staticmethod
    def _is_pdf(path: Path) -> bool:
        with path.open("rb") as stream:
            magic = stream.read(5)
        return magic == b"%PDF-" or path.suffix.lower() == ".pdf"

    @staticmethod
    def _image_content_type(path: Path) -> str:
        content_type, _ = mimetypes.guess_type(path.name)
        if content_type and content_type.startswith("image/"):
            return content_type
        return "application/octet-stream"

    async def _upload(
        self,
        path: Path,
        *,
        content_type: str,
        form: dict[str, str],
        endpoint: str,
        legacy_endpoint: str,
    ) -> httpx.Response:
        for candidate in (endpoint, legacy_endpoint):
            try:
                with path.open("rb") as stream:
                    response = await self._client.post(
                        f"{self.base_url}{candidate}",
                        data=form,
                        files={"file": (path.name, stream, content_type)},
                    )
            except OSError as error:
                raise ValueError(f"file is not readable: {path}") from error
            except httpx.HTTPError as error:
                raise OcrServiceError(f"OCR service request failed: {error}") from error

            if response.status_code != 404 or candidate == legacy_endpoint:
                return response

        raise AssertionError("OCR route fallback should always return a response")

    async def _get_with_legacy_fallback(
        self,
        endpoint: str,
        legacy_endpoint: str,
    ) -> httpx.Response:
        try:
            response = await self._client.get(f"{self.base_url}{endpoint}")
            if response.status_code == 404:
                response = await self._client.get(f"{self.base_url}{legacy_endpoint}")
            return response
        except httpx.HTTPError as error:
            raise OcrServiceError(f"OCR health request failed: {error}") from error

    def _response_payload(self, response: httpx.Response) -> dict[str, Any]:
        if not response.is_success:
            self._raise_http_error(response)
        return self._json_object(response)

    @staticmethod
    def _json_object(response: httpx.Response) -> dict[str, Any]:
        try:
            payload = response.json()
        except json.JSONDecodeError as error:
            raise OcrServiceError(
                "OCR service returned non-JSON data",
                status_code=response.status_code,
            ) from error
        if not isinstance(payload, dict):
            raise OcrServiceError(
                "OCR service returned a non-object JSON response",
                status_code=response.status_code,
            )
        return cast(dict[str, Any], payload)

    def _raise_http_error(self, response: httpx.Response) -> None:
        try:
            payload = self._json_object(response)
            message = str(payload.get("message") or payload.get("error") or "").strip()
        except OcrServiceError:
            message = response.text.strip()
        if not message:
            message = f"OCR service returned HTTP {response.status_code}"
        raise OcrServiceError(message, status_code=response.status_code)
