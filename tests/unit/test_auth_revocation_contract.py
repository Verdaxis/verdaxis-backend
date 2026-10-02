"""Contracts for administrative account-session revocation."""

from datetime import UTC, datetime, timedelta
from pathlib import Path
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import Request
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import decode_token
from app.database import Base
from app.models.refresh_session import RefreshSession
from app.models.user import Organization, User, UserRole, UserStatus
from app.routers import admin_analytics, auth_simple
from app.services.auth_revocation import invalidate_locked_user_authentication


@pytest.fixture
async def revocation_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[Organization.__table__, User.__table__, RefreshSession.__table__],
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _user(*, role=UserRole.BUYER) -> User:
    return User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="unchanged-password-hash",
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
    )


@pytest.mark.asyncio
async def test_revocation_cutoff_advances_and_preserves_password_state(revocation_db):
    user = _user()
    user.must_change_password = True
    previous = datetime.now(UTC)
    user.authentication_revoked_at = previous
    refresh_session = RefreshSession(
        user_id=user.id,
        family_id=uuid4(),
        jti_hash="a" * 64,
        device_id_hash="b" * 64,
        expires_at=previous + timedelta(days=1),
    )
    revocation_db.add_all((user, refresh_session))
    await revocation_db.commit()

    cutoff = await invalidate_locked_user_authentication(
        revocation_db,
        user,
        revoked_at=previous - timedelta(days=1),
    )
    await revocation_db.commit()

    assert cutoff == previous + timedelta(microseconds=1)
    assert user.password_hash == "unchanged-password-hash"
    assert user.password_changed_at is None
    assert user.must_change_password is True
    await revocation_db.refresh(refresh_session)
    assert refresh_session.revoked is True


def test_fresh_token_pair_is_exactly_later_than_cutoff(monkeypatch):
    cutoff = datetime.now(UTC).replace(microsecond=500_000)

    class FixedDateTime(datetime):
        @classmethod
        def now(cls, tz=None):
            return cutoff

    monkeypatch.setattr(auth_simple, "datetime", FixedDateTime)
    access_token, refresh_token = auth_simple._build_token_pair(
        "user-id", issued_after=cutoff
    )

    expected = int(cutoff.timestamp()) * 1_000_000 + cutoff.microsecond + 1
    assert decode_token(access_token)["iat_us"] == expected
    assert decode_token(refresh_token)["iat_us"] == expected


def test_authentication_revocation_migration_is_nullable_and_rollback_safe():
    column = User.__table__.columns["authentication_revoked_at"]
    assert column.nullable is True
    assert column.server_default is None
    assert _user().authentication_revoked_at is None

    root = Path(__file__).resolve().parents[2]
    migration = (
        root / "alembic/versions/auth_20261002_session_cutoff.py"
    ).read_text()
    assert 'down_revision = "catalog_20260926_biofuels"' in migration
    assert "authentication_revoked_at IS NOT NULL" in migration
    assert "op.drop_column" in migration

    checkpoints = (root / "deploy/migration-checkpoints.tsv").read_text().splitlines()
    assert (
        "catalog_20260926_biofuels	auth_20261002_session_cutoff"
        in checkpoints
    )
    assert (
        "auth_20261002_session_cutoff	auth_20261002_session_cutoff"
        in checkpoints
    )


@pytest.mark.asyncio
async def test_admin_analytics_rejection_uses_shared_revocation_helper(monkeypatch):
    user = _user()
    admin = _user(role=UserRole.ADMIN)
    user_result = MagicMock()
    user_result.scalar_one_or_none.return_value = user
    organization_result = MagicMock()
    organization_result.one_or_none.return_value = None
    db = AsyncMock()
    db.execute.side_effect = [user_result, organization_result]
    db.commit = AsyncMock()
    db.refresh = AsyncMock()
    revoke = AsyncMock()
    monkeypatch.setattr(
        admin_analytics,
        "invalidate_locked_user_authentication",
        revoke,
    )
    monkeypatch.setattr(admin_analytics, "record_status_transition", MagicMock())
    monkeypatch.setattr(admin_analytics, "record_audit", AsyncMock())
    monkeypatch.setattr(admin_analytics, "_user_to_entry", lambda row: row[0])
    request = Request(
        {
            "type": "http",
            "method": "PUT",
            "path": f"/api/admin/analytics/users/{user.id}/reject",
            "query_string": b"",
            "headers": [],
            "client": ("test", 1234),
            "scheme": "https",
            "server": ("test", 443),
        }
    )

    rejected = await admin_analytics.reject_user(
        request=request,
        user_id=user.id,
        db=db,
        current_user=admin,
    )

    assert rejected is user
    assert user.status == UserStatus.REJECTED
    revoke.assert_awaited_once_with(db, user)
