"""Exceptions raised by the OCR service Python client."""


class OcrServiceError(RuntimeError):
    """Represent a transport, HTTP, or response-contract failure.

    Attributes:
        message: Human-readable failure description.
        status_code: HTTP status when the server returned a response.
    """

    def __init__(self, message: str, *, status_code: int | None = None) -> None:
        """Initialize an OCR client failure.

        Args:
            message: Human-readable failure description.
            status_code: HTTP status when available.
        """
        super().__init__(message)
        self.message = message
        self.status_code = status_code
