"""Exceptions raised by the OCR service Python client."""


class OcrServiceError(RuntimeError):
    """Represent a transport, HTTP, or response-contract failure.

    Attributes:
        message: Human-readable failure description.
        status_code: HTTP status when the server returned a response.
        cloud_attempted: Whether a request to Jina may have been made while serving the
            request; None when the response did not say.
    """

    def __init__(
        self,
        message: str,
        *,
        status_code: int | None = None,
        cloud_attempted: bool | None = None,
    ) -> None:
        """Initialize an OCR client failure.

        Args:
            message: Human-readable failure description.
            status_code: HTTP status when available.
            cloud_attempted: Whether Jina may have been called, when the server said.
        """
        super().__init__(message)
        self.message = message
        self.status_code = status_code
        self.cloud_attempted = cloud_attempted
