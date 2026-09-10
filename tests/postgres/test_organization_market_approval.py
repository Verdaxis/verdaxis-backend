"""Company approval classifies live organizations without wider app privileges."""

import os
from uuid import UUID, uuid4

import pytest
from fastapi import Request
from sqlalchemy import select, text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.models.audit import AuditLog
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.routers.auth_simple import approve_organization


@pytest.mark.asyncio
async def test_company_approval_classifies_and_audits_with_app_role(pg_session):
    _, seed = pg_session
    organization = Organization(id=uuid4(), name="Customer", type=OrgType.FUEL_BUYER)
    admin = User(id=uuid4(), email=f"{uuid4()}@example.test", password_hash="hash",
                 role=UserRole.ADMIN, status=UserStatus.APPROVED, email_verified=True)
    seed.add_all([organization, admin])
    await seed.commit()

    app_engine = create_async_engine(os.environ["DATABASE_URL"])
    try:
        factory = async_sessionmaker(app_engine, expire_on_commit=False)
        async with factory() as session:
            await approve_organization(
                request=Request({"type": "http", "method": "PUT",
                                 "path": "/api/auth/organization/approve", "headers": [],
                                 "client": ("127.0.0.1", 1234)}),
                organization_id=organization.id, current_user=admin, db=session,
            )
            current = await session.get(Organization, organization.id)
            assert current.provenance == "REAL"
            audits = (await session.scalars(select(AuditLog))).all()
            assert len(audits) == 1
            assert audits[0].changes["provenance"] == {"from": "UNKNOWN", "to": "REAL"}
            assert audits[0].user_id == admin.id
            await session.rollback()
            with pytest.raises(DBAPIError, match="permission denied"):
                await session.execute(text("UPDATE organizations SET provenance='UNKNOWN' WHERE id=:id"),
                                      {"id": organization.id})
            await session.rollback()
    finally:
        await app_engine.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize("organization_id,provenance,expected", [
    (uuid4(), "UNKNOWN", "REAL"),
    (UUID("4da7b285-34ee-5443-9406-f96b4ed1a251"), "DEMO", "DEMO"),
    (UUID("9e63f7a1-0000-4000-8000-000000000001"), "TEST", "TEST"),
    (UUID("4da7b285-34ee-5443-9406-f96b4ed1a251"), "UNKNOWN", "UNKNOWN"),
    (UUID("9e63f7a1-0000-4000-8000-000000000001"), "UNKNOWN", "UNKNOWN"),
])
async def test_classification_preserves_synthetic_identity_and_rolls_back(
    pg_session, organization_id, provenance, expected,
):
    _, session = pg_session
    session.add(Organization(id=organization_id, name="Approval fixture", type=OrgType.FUEL_BUYER,
                             provenance=provenance))
    await session.commit()
    statement = text("UPDATE organizations SET verification_status='APPROVED' WHERE id=:id RETURNING provenance")
    result = await session.execute(statement, {"id": organization_id})
    assert result.scalar_one() == expected
    await session.rollback()
    assert (await session.execute(text("SELECT provenance FROM organizations WHERE id=:id"),
                                  {"id": organization_id})).scalar_one() == provenance


@pytest.mark.asyncio
async def test_historical_approval_and_rejection_do_not_auto_promote(pg_session):
    _, session = pg_session
    organization_id = uuid4()
    session.add(Organization(id=organization_id, name="Historical", type=OrgType.FUEL_BUYER,
                             verification_status="APPROVED"))
    await session.commit()
    for assignment in ("name='Renamed'", "verification_status='APPROVED'", "verification_status='REJECTED'"):
        result = await session.execute(text(f"UPDATE organizations SET {assignment} WHERE id=:id RETURNING provenance"),
                                       {"id": organization_id})
        assert result.scalar_one() == "UNKNOWN"
    await session.commit()
    with pytest.raises(DBAPIError, match="provenance is immutable"):
        await session.execute(text("UPDATE organizations SET provenance='REAL', verification_status='APPROVED' WHERE id=:id"),
                              {"id": organization_id})
    await session.rollback()
