from io import BytesIO
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import FastAPI, HTTPException, UploadFile
from httpx import ASGITransport, AsyncClient

from app.database import get_db
from app.routers import kyc as kyc_router
from app.routers.auth_simple import get_current_user
from app.routers.kyc import read_bounded_kyc_documents, read_bounded_upload


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


class _FakeSession:
    async def commit(self):
        pass


def _kyc_test_app(monkeypatch, *, max_bytes: int = 8):
    app = FastAPI()
    app.include_router(kyc_router.router, prefix="/api")
    user = SimpleNamespace(
        id=uuid4(),
        email="test@example.com",
        first_name="Test",
        kyc_status="PENDING",
        kyc_rejection_reason=None,
    )
    app.dependency_overrides[get_current_user] = lambda: user
    app.dependency_overrides[get_db] = lambda: _FakeSession()
    monkeypatch.setattr(kyc_router.settings, "KYC_MAX_FILE_BYTES", max_bytes)
    monkeypatch.setattr(kyc_router.settings, "KYC_MAX_TOTAL_BYTES", max_bytes * 2)
    async def verify(*args, **kwargs):
        return {"passed": True, "issues": []}
    async def audit(*args, **kwargs):
        return None
    async def email(*args, **kwargs):
        return True
    monkeypatch.setattr(kyc_router, "verify_document_with_gemini", verify)
    monkeypatch.setattr(kyc_router, "record_audit", audit)
    monkeypatch.setattr(kyc_router, "send_kyc_approved_email", email)
    return app


@pytest.mark.asyncio
async def test_kyc_upload_is_read_only_within_per_file_limit():
    upload = UploadFile(filename="passport.jpg", file=BytesIO(b"passport"))

    assert await read_bounded_upload(upload, max_bytes=8) == b"passport"


@pytest.mark.asyncio
async def test_kyc_upload_rejects_oversize_before_returning_bytes():
    upload = UploadFile(filename="passport.jpg", file=BytesIO(b"123456789"))

    with pytest.raises(HTTPException) as exc_info:
        await read_bounded_upload(upload, max_bytes=8)

    assert exc_info.value.status_code == 413


@pytest.mark.asyncio
async def test_kyc_documents_reject_aggregate_limit_before_reading_second_file():
    passport = UploadFile(filename="passport.jpg", file=BytesIO(b"123456"))
    company_doc = UploadFile(filename="company.pdf", file=BytesIO(b"12345"))

    with pytest.raises(HTTPException) as exc_info:
        await read_bounded_kyc_documents(
            passport,
            company_doc,
            max_file_bytes=8,
            max_total_bytes=10,
        )

    assert exc_info.value.status_code == 413


@pytest.mark.asyncio
async def test_multipart_upload_without_content_length_still_enforces_streaming_limit(monkeypatch):
    app = _kyc_test_app(monkeypatch, max_bytes=8)
    boundary = "runtime-hardening-missing-length"
    body = _multipart_body(
        {
            "passport": ("passport.jpg", b"123456789", "image/jpeg"),
            "company_doc": ("company.pdf", b"1234", "application/pdf"),
        },
        boundary,
    )

    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://testserver"
    ) as client:
        response = await client.post(
            "/api/kyc/submit",
            content=body,
            headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
        )

    assert response.status_code == 413


@pytest.mark.asyncio
async def test_multipart_upload_with_lying_content_length_uses_actual_stream_size(monkeypatch):
    app = _kyc_test_app(monkeypatch, max_bytes=8)
    boundary = "runtime-hardening-lying-length"
    body = _multipart_body(
        {
            "passport": ("passport.jpg", b"123456789", "image/jpeg"),
            "company_doc": ("company.pdf", b"1234", "application/pdf"),
        },
        boundary,
    )

    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="http://testserver"
    ) as client:
        response = await client.post(
            "/api/kyc/submit",
            content=body,
            headers={
                "Content-Type": f"multipart/form-data; boundary={boundary}",
                "Content-Length": "1",
            },
        )

    assert response.status_code == 413
