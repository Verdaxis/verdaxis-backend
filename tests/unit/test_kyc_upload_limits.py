from io import BytesIO

import pytest
from fastapi import HTTPException, UploadFile

from app.routers.kyc import read_bounded_kyc_documents, read_bounded_upload


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
