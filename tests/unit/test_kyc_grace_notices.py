"""The one-time KYC grace notice CLI and its idempotent delivery."""
from datetime import UTC, datetime, timedelta
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.cli import send_kyc_grace_notices as cli
from app.database import Base
from app.models.audit import AuditLog
from app.models.kyc_execution_grace import KycExecutionGrace
from app.models.user import Organization, OrganizationProvenance, OrgType, User, UserRole, UserStatus
from app.services import kyc_grace_notices
from app.services.audit_actions import KYC_GRACE_NOTICE_SENT

ENROLLED_AT = datetime(2026, 10, 14, 12, 0, tzinfo=UTC)
GRACE_EXPIRES_AT = ENROLLED_AT + timedelta(days=14)
DURING_GRACE = ENROLLED_AT + timedelta(days=1)
RELEASE_SHA = "a" * 40


@pytest.fixture
async def db():
    engine = create_async_engine("sqlite+aiosqlite://")
    tables = [
        Base.metadata.tables[name]
        for name in ("organizations", "users", "kyc_execution_grace", "audit_logs")
    ]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


async def _graced_user(db: AsyncSession, *, kyc_status: str) -> User:
    organization = Organization(
        name=f"Org {uuid4().hex[:8]}",
        type=OrgType.FUEL_BUYER,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    db.add(organization)
    await db.flush()
    user = User(
        email=f"{uuid4().hex}@notice.test",
        password_hash="unused",
        first_name="Ada",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        organization_id=organization.id,
        kyc_status=kyc_status,
        kyc_organization_id=organization.id if kyc_status == "APPROVED" else None,
    )
    db.add(user)
    await db.flush()
    db.add(
        KycExecutionGrace(
            user_id=user.id,
            organization_id=organization.id,
            kyc_status_at_enrollment="PENDING",
            enrolled_at=ENROLLED_AT,
            expires_at=GRACE_EXPIRES_AT,
            created_by="kycg_20261014_prod",
        )
    )
    await db.commit()
    return user


@pytest.mark.parametrize(
    "argv",
    [
        [],
        ["--release-sha", RELEASE_SHA],
        ["--environment", "development", "--release-sha", RELEASE_SHA],
        ["--environment", "production", "--release-sha", "abc123"],
    ],
)
def test_cli_requires_an_explicit_deployed_identity(argv):
    with pytest.raises(SystemExit):
        cli.parse_cli_args(argv)


def test_cli_refuses_a_runtime_that_is_not_the_named_target():
    with pytest.raises(RuntimeError):
        cli.assert_runtime_identity(
            configured_environment="staging",
            configured_release_sha=RELEASE_SHA,
            expected_environment="production",
            expected_release_sha=RELEASE_SHA,
        )
    with pytest.raises(RuntimeError):
        cli.assert_runtime_identity(
            configured_environment="production",
            configured_release_sha="b" * 40,
            expected_environment="production",
            expected_release_sha=RELEASE_SHA,
        )


@pytest.mark.asyncio
async def test_each_grace_row_gets_exactly_one_notice(db, monkeypatch):
    graced = await _graced_user(db, kyc_status="PENDING")
    await _graced_user(db, kyc_status="APPROVED")
    deliver = AsyncMock(return_value=True)
    monkeypatch.setattr(kyc_grace_notices, "send_kyc_grace_notice_email", deliver)

    first = await kyc_grace_notices.send_kyc_grace_notices(db, now=DURING_GRACE)
    second = await kyc_grace_notices.send_kyc_grace_notices(db, now=DURING_GRACE)

    # The approved user no longer depends on grace, so no notice is due.
    assert first == {"sent": 1, "skipped": 1, "failed": 0}
    assert second == {"sent": 0, "skipped": 1, "failed": 0}
    deliver.assert_awaited_once()
    payload, grace_id = deliver.await_args.args
    assert payload["to"] == [graced.email]
    assert payload["subject"] == "Complete your KYC to keep trading on Verdaxis"
    assert "/kyc" in payload["html"]
    assert "28 October 2026 at 12:00" in payload["html"]
    records = (
        await db.execute(select(AuditLog).where(AuditLog.action == KYC_GRACE_NOTICE_SENT))
    ).scalars().all()
    assert [record.resource_id for record in records] == [str(grace_id)]


@pytest.mark.asyncio
async def test_a_failed_send_is_not_recorded_and_is_retried(db, monkeypatch):
    await _graced_user(db, kyc_status="SUBMITTED")
    deliver = AsyncMock(side_effect=[False, True])
    monkeypatch.setattr(kyc_grace_notices, "send_kyc_grace_notice_email", deliver)

    assert await kyc_grace_notices.send_kyc_grace_notices(db, now=DURING_GRACE) == {
        "sent": 0, "skipped": 0, "failed": 1,
    }
    assert await kyc_grace_notices.send_kyc_grace_notices(db, now=DURING_GRACE) == {
        "sent": 1, "skipped": 0, "failed": 0,
    }
    # The provider key is bound to the grace row, so a provider-side retry
    # after an uncertain send cannot deliver twice either.
    first_key = deliver.await_args_list[0].args[1]
    assert deliver.await_args_list[1].args[1] == first_key
