from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine

from app.cli.send_kyc_grace_notices import assert_runtime_identity, parse_cli_args, send_notices
from app.database import Base
from app.models.kyc_execution_grace import KYCExecutionGrace, KYCGraceNotice
from app.models.user import Organization, User, UserRole, UserStatus, OrgType


def test_notice_cli_requires_explicit_deployed_identity():
    with pytest.raises(SystemExit):
        parse_cli_args([])
    with pytest.raises(SystemExit):
        parse_cli_args(["--environment", "staging", "--release-sha", "head"])
    settings = SimpleNamespace(ENVIRONMENT="staging", RELEASE_SHA="a" * 40)
    assert_runtime_identity(settings, "staging", "a" * 40)
    for environment, sha in (("production", "a" * 40), ("staging", "b" * 40), ("development", "a" * 40)):
        with pytest.raises(RuntimeError):
            assert_runtime_identity(settings, environment, sha)


@pytest.mark.asyncio
@pytest.mark.parametrize("delivered", [True, False])
async def test_notice_claim_is_not_sent_twice_even_after_uncertain_result(monkeypatch, delivered):
    engine = create_async_engine("sqlite+aiosqlite:///:memory:")
    tables = [Organization.__table__, User.__table__, KYCExecutionGrace.__table__, KYCGraceNotice.__table__]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
    sender = AsyncMock(return_value=delivered)
    monkeypatch.setattr("app.services.email.send_kyc_grace_notice", sender)
    try:
        async with async_sessionmaker(engine, expire_on_commit=False)() as db:
            org_id, user_id, grace_id = uuid4(), uuid4(), uuid4()
            now = datetime.now(UTC)
            db.add(Organization(id=org_id, name="Notice test", type=OrgType.FUEL_BUYER))
            db.add(User(id=user_id, email="grace@example.test", password_hash="unused", role=UserRole.BUYER,
                        status=UserStatus.APPROVED, organization_id=org_id, kyc_status="APPROVED", kyc_organization_id=None))
            db.add(KYCExecutionGrace(id=grace_id, user_id=user_id, organization_id=org_id,
                                    kyc_status_at_enrollment="APPROVED", enrolled_at=now,
                                    expires_at=now + timedelta(days=14), created_by="kycg_20261014_staging"))
            await db.commit()
            assert await send_notices(db) == ((1, 0) if delivered else (0, 1))
            assert await send_notices(db) == (0, 0)
            sender.assert_awaited_once()
            notice = await db.get(KYCGraceNotice, grace_id)
            assert (notice.sent_at is not None) == delivered
    finally:
        await engine.dispose()
