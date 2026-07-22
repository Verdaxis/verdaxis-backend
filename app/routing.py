"""Small route-level transport guards shared by sensitive endpoints."""

from urllib.parse import urlsplit

from fastapi import HTTPException, Request, status
from fastapi.routing import APIRoute

from app.config import credentialed_origins_for_environment


def require_trusted_browser_origin(
    request: Request,
    *,
    environment: str,
    cookie_authenticated: bool,
) -> None:
    """Enforce exact Origin semantics for browser cookie transitions."""
    origin = request.headers.get("origin")
    if origin is None:
        if not cookie_authenticated:
            return
    else:
        try:
            parsed = urlsplit(origin)
            canonical = f"{parsed.scheme.lower()}://{parsed.netloc.lower()}"
        except ValueError:
            canonical = ""
            parsed = urlsplit("")
        if (
            parsed.scheme.lower() in {"http", "https"}
            and parsed.netloc
            and not parsed.path
            and not parsed.query
            and not parsed.fragment
            and canonical in credentialed_origins_for_environment(environment)
        ):
            return
    raise HTTPException(
        status_code=status.HTTP_403_FORBIDDEN,
        detail={
            "code": "BROWSER_ORIGIN_REJECTED",
            "message": "Browser origin is not allowed for this authentication operation",
        },
    )


class BodySizeLimitRoute(APIRoute):
    """Reject oversized bodies before FastAPI parses JSON or multipart data."""

    max_body_bytes = 0
    body_too_large_detail = "Request body is too large"

    def get_route_handler(self):
        original_handler = super().get_route_handler()

        async def limited_handler(request: Request):
            content_length = request.headers.get("content-length")
            if content_length:
                try:
                    if int(content_length) > self.max_body_bytes:
                        raise HTTPException(
                            status_code=status.HTTP_413_CONTENT_TOO_LARGE,
                            detail=self.body_too_large_detail,
                        )
                except ValueError as exc:
                    raise HTTPException(status_code=400, detail="Invalid Content-Length header") from exc

            received = 0
            original_receive = request.receive

            async def limited_receive():
                nonlocal received
                message = await original_receive()
                if message["type"] == "http.request":
                    received += len(message.get("body", b""))
                    if received > self.max_body_bytes:
                        raise HTTPException(
                            status_code=status.HTTP_413_CONTENT_TOO_LARGE,
                            detail=self.body_too_large_detail,
                        )
                return message

            limited_request = Request(request.scope, receive=limited_receive)
            return await original_handler(limited_request)

        return limited_handler
