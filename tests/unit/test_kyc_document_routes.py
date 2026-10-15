"""KYC retention and download transactions with real SQLite sessions."""
from io import BytesIO
from types import SimpleNamespace
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request, UploadFile
from sqlalchemy import select
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine
from starlette.datastructures import Headers

from app.database import Base
from app.models.audit import AuditLog
from app.models.kyc_document import KYCDocument
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.routers import kyc
from app.services.kyc_document_vault import KeyRing


@pytest.fixture
async def state(monkeypatch):
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=[Organization.__table__, User.__table__, AuditLog.__table__, KYCDocument.__table__])
    async with async_sessionmaker(engine, expire_on_commit=False)() as db:
        org = Organization(id=uuid4(), name="Customer", type=OrgType.FUEL_BUYER)
        user = User(id=uuid4(), email="vault@example.com", password_hash="unused", role=UserRole.BUYER,
                    status=UserStatus.APPROVED, email_verified=True, must_change_password=False,
                    organization_id=org.id, kyc_status="PENDING")
        admin = User(id=uuid4(), email="admin@example.com", password_hash="unused", role=UserRole.ADMIN,
                     status=UserStatus.APPROVED)
        db.add_all([org, user, admin])
        await db.commit()
        ring = KeyRing("test", {"test": b"t" * 32})
        request = Request({"type": "http", "method": "POST", "path": "/api/kyc/submit",
                           "headers": [], "client": ("127.0.0.1", 4321),
                           "app": SimpleNamespace(state=SimpleNamespace(kyc_document_keys=ring))})
        monkeypatch.setattr(kyc, "verify_document_with_gemini", AsyncMock(return_value={"passed": True}))
        monkeypatch.setattr(kyc.limiter, "enabled", False)
        yield db, user, admin, request
    await engine.dispose()


def upload():
    return UploadFile(file=BytesIO(b"%PDF-private"), filename="private-name.pdf", headers=Headers({"content-type": "application/pdf"}))


async def submit(state):
    db, user, _, request = state
    result = await kyc.submit_kyc(request=request, current_user=user, db=db, passport=upload(), company_doc=upload(),
                                  declared_company_name=None, registration_number=None)
    # A new authenticated request reloads its actor after submit's rollback.
    await db.refresh(state[2])
    return result


@pytest.mark.parametrize("unavailable", [False, True])
async def test_submit_retains_two_documents_and_history(state, monkeypatch, unavailable):
    if unavailable:
        monkeypatch.setattr(kyc, "verify_document_with_gemini", AsyncMock(side_effect=kyc.KYCProviderUnavailable()))
    db, user, _, request = state
    await submit(state)
    rows = (await db.scalars(select(KYCDocument))).all()
    assert len(rows) == 2
    assert {row.kind for row in rows} == {"IDENTITY", "COMPANY_REGISTRATION"}
    assert len({row.submission_id for row in rows}) == 1
    assert all(row.organization_id == user.organization_id for row in rows)
    assert all(request.app.state.kyc_document_keys.decrypt(row) == b"%PDF-private" for row in rows)
    audit = (await db.scalars(select(AuditLog))).one()
    assert audit.changes["advisory"] == {"provider": "none", "status": "disabled"}
    kyc.verify_document_with_gemini.assert_not_called()
    assert audit.changes["raw_documents_retained_in_platform"] is True
    assert set(audit.changes["document_ids"]) == {str(row.id) for row in rows}
    assert audit.changes["submission_id"] == str(rows[0].submission_id)
    assert "private" not in str(audit.changes)
    await submit(state)
    assert len((await db.scalars(select(KYCDocument))).all()) == 4


async def test_missing_keys_changes_nothing(state):
    db, user, _, request = state
    user_id = user.id
    request.app.state.kyc_document_keys = None
    with pytest.raises(HTTPException) as error:
        await submit(state)
    assert error.value.status_code == 503
    assert error.value.detail["code"] == "KYC_DOCUMENT_STORAGE_UNAVAILABLE"
    await db.rollback()
    assert (await db.get(User, user_id)).kyc_status == "PENDING"
    assert not (await db.scalars(select(KYCDocument))).all()
    assert not (await db.scalars(select(AuditLog))).all()


async def test_submit_audit_failure_rolls_back_all(state, monkeypatch):
    db, user, _, _ = state
    user_id = user.id
    monkeypatch.setattr(kyc, "record_audit", AsyncMock(side_effect=RuntimeError("audit unavailable")))
    with pytest.raises(RuntimeError):
        await submit(state)
    await db.rollback()
    assert (await db.get(User, user_id)).kyc_status == "PENDING"
    assert not (await db.scalars(select(KYCDocument))).all()


async def test_admin_list_download_and_audit(state):
    db, user, admin, request = state
    await submit(state)
    result = await kyc.admin_list_kyc_documents(user.id, request, admin, db)
    assert len(result) == 2
    assert set(result[0]) == {"id", "submission_id", "kind", "content_type", "size_bytes", "organization_id", "created_at"}
    rows = (await db.scalars(select(KYCDocument))).all()
    response = await kyc.admin_download_kyc_document(rows[0].id, request, admin, db)
    assert response.body == b"%PDF-private"
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["x-content-type-options"] == "nosniff"
    assert response.headers["content-disposition"] == f'attachment; filename="kyc-{rows[0].kind}-{rows[0].id}.pdf"'
    audit = (await db.scalars(select(AuditLog).where(AuditLog.action == "kyc.document_viewed"))).one()
    assert audit.user_id == admin.id
    assert audit.changes["user_id"] == str(user.id)
    assert audit.resource_id == str(rows[0].id)


async def test_admin_authorization_and_unknown_ids(state):
    db, user, admin, request = state
    for fn in (kyc.admin_list_kyc_documents, kyc.admin_download_kyc_document):
        with pytest.raises(HTTPException) as error:
            await fn(uuid4(), request, user, db)
        assert error.value.status_code == 403
        with pytest.raises(HTTPException) as error:
            await fn(uuid4(), request, admin, db)
        assert error.value.status_code == 404


async def test_download_audit_failure_returns_no_bytes(state, monkeypatch):
    db, _, admin, request = state
    await submit(state)
    row = (await db.scalars(select(KYCDocument))).first()
    monkeypatch.setattr(kyc, "record_audit", AsyncMock(side_effect=RuntimeError("audit unavailable")))
    with pytest.raises(RuntimeError):
        await kyc.admin_download_kyc_document(row.id, request, admin, db)


async def test_corrupt_document_returns_sanitized_error(state, caplog):
    db, _, admin, request = state
    await submit(state)
    row = (await db.scalars(select(KYCDocument))).first()
    row.ciphertext = b"invalid-private-data"
    with pytest.raises(HTTPException) as error:
        await kyc.admin_download_kyc_document(row.id, request, admin, db)
    assert error.value.status_code == 500
    assert error.value.detail == {"code": "KYC_DOCUMENT_DECRYPTION_FAILED"}
    assert "invalid-private-data" not in caplog.text
    assert "tttttttt" not in caplog.text
