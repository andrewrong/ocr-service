"""Typed results returned by the OCR service Python client."""

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class OcrResult:
    """Recognized Markdown and execution metadata."""

    markdown: str
    engine: str
    pages: int
    duration_ms: int


@dataclass(frozen=True, slots=True)
class ModelStatus:
    """Availability of one configured OCR engine."""

    engine: str
    name: str
    available: bool


@dataclass(frozen=True, slots=True)
class JinaHealth:
    """Cloud configuration and metadata reachability, not credential validation."""

    configured: bool
    reachable: bool


@dataclass(frozen=True, slots=True)
class HealthResult:
    """OCR service readiness and model availability."""

    status: str
    ollama: bool
    models: tuple[ModelStatus, ...]
    backend: str = "ollama"
    backend_ready: bool | None = None
    jina: JinaHealth | None = None

    @property
    def ready(self) -> bool:
        """Return whether at least one OCR model can accept requests."""
        return self.status == "ok" and any(model.available for model in self.models)
