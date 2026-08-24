"""Tests for the public OCR client interface."""

from pathlib import Path

import httpx
import pytest

from ocr_service_client import OcrClient, OcrServiceError


@pytest.mark.asyncio
async def test_recognize_image_hides_multipart_and_route_selection(tmp_path: Path) -> None:
    """Route image files to the versioned image endpoint and parse the result."""
    image = tmp_path / "receipt.png"
    image.write_bytes(b"\x89PNG\r\n\x1a\nimage")

    async def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/ocr/image"
        body = await request.aread()
        assert b'name="engine"' in body
        assert b"auto" in body
        assert b'name="file"' in body
        assert b"page_range" not in body
        return httpx.Response(
            200,
            json={
                "markdown": "# Receipt",
                "engine": "paddle",
                "pages": 1,
                "duration_ms": 321,
            },
        )

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test", http_client=http_client)
        result = await client.recognize(image)

    assert result.markdown == "# Receipt"
    assert result.engine == "paddle"
    assert result.pages == 1
    assert result.duration_ms == 321


@pytest.mark.asyncio
async def test_recognize_pdf_routes_pages_to_pdf_endpoint(tmp_path: Path) -> None:
    """Detect PDFs by content and send an optional inclusive page range."""
    document = tmp_path / "scan.bin"
    document.write_bytes(b"%PDF-1.7\ncontent")

    async def handler(request: httpx.Request) -> httpx.Response:
        assert request.url.path == "/v1/ocr/pdf"
        body = await request.aread()
        assert b'name="page_range"' in body
        assert b"2-7" in body
        return httpx.Response(
            200,
            json={
                "markdown": "## Page 2",
                "engine": "mixed",
                "pages": 6,
                "duration_ms": 12_000,
            },
        )

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test/", http_client=http_client)
        result = await client.recognize(document, engine="glm", pages="2-7")

    assert result.engine == "mixed"
    assert result.pages == 6


@pytest.mark.asyncio
async def test_recognize_rejects_pages_for_an_image_before_network_call(tmp_path: Path) -> None:
    """Reject PDF-only options locally with a clear error."""
    image = tmp_path / "photo.jpg"
    image.write_bytes(b"jpeg")

    async def unexpected_request(request: httpx.Request) -> httpx.Response:
        pytest.fail(f"unexpected request to {request.url}")

    transport = httpx.MockTransport(unexpected_request)
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test", http_client=http_client)
        with pytest.raises(ValueError, match="pages is only valid for PDF"):
            await client.recognize(image, pages="1-2")


@pytest.mark.asyncio
async def test_server_error_becomes_typed_exception(tmp_path: Path) -> None:
    """Preserve the HTTP status and server message in a typed exception."""
    image = tmp_path / "photo.png"
    image.write_bytes(b"png")

    transport = httpx.MockTransport(
        lambda request: httpx.Response(502, json={"error": "all OCR engines timed out"})
    )
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test", http_client=http_client)
        with pytest.raises(OcrServiceError) as captured:
            await client.recognize(image)

    assert captured.value.status_code == 502
    assert captured.value.message == "all OCR engines timed out"


@pytest.mark.asyncio
async def test_health_returns_model_availability() -> None:
    """Expose health details without leaking the wire response shape to callers."""
    transport = httpx.MockTransport(
        lambda request: httpx.Response(
            200,
            json={
                "status": "ok",
                "ollama": True,
                "models": [
                    {"engine": "paddle", "name": "paddle-model", "available": True},
                    {"engine": "glm", "name": "glm-model", "available": False},
                ],
            },
        )
    )
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test", http_client=http_client)
        health = await client.health()

    assert health.ready is True
    assert health.models[0].engine == "paddle"
    assert health.models[1].available is False


@pytest.mark.asyncio
async def test_recognize_falls_back_to_legacy_route_during_deployment_upgrade(
    tmp_path: Path,
) -> None:
    """Keep the SDK usable while a running service still exposes only legacy routes."""
    image = tmp_path / "photo.png"
    image.write_bytes(b"png")
    requested_paths: list[str] = []

    async def handler(request: httpx.Request) -> httpx.Response:
        requested_paths.append(request.url.path)
        if request.url.path == "/v1/ocr/image":
            return httpx.Response(404)
        return httpx.Response(
            200,
            json={
                "markdown": "legacy result",
                "engine": "glm",
                "pages": 1,
                "duration_ms": 100,
            },
        )

    transport = httpx.MockTransport(handler)
    async with httpx.AsyncClient(transport=transport) as http_client:
        client = OcrClient("http://ocr.test", http_client=http_client)
        result = await client.recognize(image)

    assert requested_paths == ["/v1/ocr/image", "/ocr/image"]
    assert result.markdown == "legacy result"
