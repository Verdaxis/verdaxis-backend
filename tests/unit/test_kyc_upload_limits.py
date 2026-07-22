"""KYC upload size limits stay config-wired and enforced on real streams.

Restores the runtime branch's upload-limit coverage, adapted to the
security implementation that landed on the integration tree
(`_read_validated_document` + `_KYCBodyLimitRoute`).
"""
from io import BytesIO

import pytest
from fastapi import APIRouter, FastAPI, File, HTTPException, UploadFile
from httpx import ASGITransport, AsyncClient

from app.config import settings
from app.routers import kyc as kyc_router


def _upload(content: bytes, content_type: str) -> UploadFile:
    return UploadFile(
        filename="doc",
        file=BytesIO(content),
        headers={"content-type": content_type},
    )


def _multipart_body(files: dict[str, tuple[str, bytes, str]], boundary: str) -> bytes:
    parts = []
    for field, (filename, content, content_type) in files.items():
        parts.extend(
            [
                f"--{boundary}\r\n".encode(),
                f'Content-Disposition: form-data; name="{field}"; filename="{filename}"\r\n'.encode(),
                f"Content-Type: {content_type}\r\n\r\n".encode(),
                content,
                b"\r\n",
            ]
        )
    parts.append(f"--{boundary}--\r\n".encode())
    return b"".join(parts)


def test_kyc_limits_are_wired_to_runtime_settings():
    assert kyc_router.MAX_KYC_DOCUMENT_BYTES == settings.KYC_MAX_FILE_BYTES
    assert kyc_router.MAX_KYC_REQUEST_BODY_BYTES == settings.KYC_MAX_TOTAL_BYTES + 64 * 1024


@pytest.mark.asyncio
async def test_document_within_limit_is_read_and_returned(monkeypatch):
    monkeypatch.setattr(kyc_router, "MAX_KYC_DOCUMENT_BYTES", 16)

    content, mime = await kyc_router._read_validated_document(
        _upload(b"\xff\xd8\xffpassport", "image/jpeg")
    )

    assert content == b"\xff\xd8\xffpassport"
    assert mime == "image/jpeg"


@pytest.mark.asyncio
async def test_oversize_document_rejected_before_returning_bytes(monkeypatch):
    monkeypatch.setattr(kyc_router, "MAX_KYC_DOCUMENT_BYTES", 8)

    with pytest.raises(HTTPException) as exc_info:
        await kyc_router._read_validated_document(
            _upload(b"\xff\xd8\xff123456789", "image/jpeg")
        )

    assert exc_info.value.status_code == 413


def _body_limit_test_app(monkeypatch, *, max_body_bytes: int) -> FastAPI:
    """Mount a minimal endpoint behind the real KYC body-limit route class."""
    monkeypatch.setattr(kyc_router._KYCBodyLimitRoute, "max_body_bytes", max_body_bytes)
    router = APIRouter(route_class=kyc_router._KYCBodyLimitRoute)

    @router.post("/upload")
    async def upload(passport: UploadFile = File(...)):
        content = await passport.read()
        return {"received": len(content)}

    app = FastAPI()
    app.include_router(router)
    return app


@pytest.mark.asyncio
async def test_multipart_upload_without_content_length_still_enforces_streaming_limit(monkeypatch):
    app = _body_limit_test_app(monkeypatch, max_body_bytes=64)
    boundary = "integration-hardening-missing-length"
    body = _multipart_body(
        {"passport": ("passport.jpg", b"\xff\xd8\xff" + b"x" * 128, "image/jpeg")},
        boundary,
    )

    seen_headers = {}

    async def chunked_body():
        for offset in range(0, len(body), 7):
            yield body[offset : offset + 7]

    @app.middleware("http")
    async def capture_headers(request, call_next):
        seen_headers.update(request.headers)
        return await call_next(request)

    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://testserver"
    ) as client:
        response = await client.post(
            "/upload",
            content=chunked_body(),
            headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
        )

    assert response.status_code == 413
    assert "content-length" not in seen_headers
    assert seen_headers["transfer-encoding"] == "chunked"


@pytest.mark.asyncio
async def test_multipart_upload_with_lying_content_length_uses_actual_stream_size(monkeypatch):
    app = _body_limit_test_app(monkeypatch, max_body_bytes=64)
    boundary = "integration-hardening-lying-length"
    body = _multipart_body(
        {"passport": ("passport.jpg", b"\xff\xd8\xff" + b"x" * 128, "image/jpeg")},
        boundary,
    )

    async def chunked_body():
        for offset in range(0, len(body), 5):
            yield body[offset : offset + 5]

    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://testserver"
    ) as client:
        response = await client.post(
            "/upload",
            content=chunked_body(),
            headers={
                "Content-Type": f"multipart/form-data; boundary={boundary}",
                "Content-Length": "1",
            },
        )

    assert response.status_code == 413
