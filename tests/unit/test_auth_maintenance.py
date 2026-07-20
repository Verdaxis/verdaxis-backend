"""Bounded cleanup contracts for authentication state."""

from datetime import UTC, datetime, timedelta
from uuid import uuid4

import pytest
from sqlalchemy import select, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.refresh_session import RefreshSession
from app.models.registration import PendingRegistration
from app.models.user import Organization, User, UserRole, UserStatus
from app.services.auth_maintenance import run_auth_maintenance


@pytest.fixture
async def maintenance_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    tables = [
        Organization.__table__,
        User.__table__,
        RefreshSession.__table__,
        PendingRegistration.__table__,
    ]
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all, tables=tables)
        await connection.execute(
            text("ALTER TABLE users ADD COLUMN email_verification_token VARCHAR")
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


@pytest.mark.asyncio
async def test_auth_maintenance_cleans_each_category_without_losing_replay_state(
    maintenance_db: AsyncSession,
):
    now = datetime.now(UTC)
    user = User(
        id=uuid4(),
        email=f"{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=False,
        password_reset_token_hash="r" * 64,
        password_reset_expires=None,
        email_verification_token_hash="e" * 64,
        email_verification_token_expires_at=None,
    )
    maintenance_db.add(user)
    await maintenance_db.flush()

    expired_family = uuid4()
    live_family = uuid4()
    maintenance_db.add_all(
        [
            RefreshSession(
                user_id=user.id,
                family_id=expired_family,
                jti_hash="1" * 64,
                expires_at=now - timedelta(seconds=1),
            ),
            RefreshSession(
                user_id=user.id,
                family_id=live_family,
                jti_hash="2" * 64,
                replaced_by_jti_hash="3" * 64,
                revoked=True,
                created_at=now - timedelta(days=2),
                expires_at=now + timedelta(days=1),
            ),
            RefreshSession(
                user_id=user.id,
                family_id=live_family,
                jti_hash="3" * 64,
                revoked=False,
                expires_at=now + timedelta(days=1),
            ),
            PendingRegistration(
                token_hash="4" * 64,
                email="expired@example.test",
                password_hash="hash",
                expires_at=now - timedelta(seconds=1),
            ),
            PendingRegistration(
                token_hash="5" * 64,
                email="consumed@example.test",
                password_hash="hash",
                expires_at=now + timedelta(minutes=5),
                used_at=now,
            ),
            PendingRegistration(
                token_hash="6" * 64,
                email="live@example.test",
                password_hash="hash",
                expires_at=now + timedelta(minutes=5),
            ),
        ]
    )
    await maintenance_db.execute(
        text(
            "UPDATE users SET email_verification_token = 'legacy-plaintext' "
            "WHERE id = :user_id"
        ),
        {"user_id": user.id.hex},
    )
    await maintenance_db.commit()

    report = await run_auth_maintenance(maintenance_db, now=now, batch_size=10)
    await maintenance_db.commit()

    assert report == {
        "refresh_sessions_deleted": 1,
        "pending_registrations_deleted": 2,
        "password_reset_hashes_cleared": 1,
        "email_verification_hashes_cleared": 1,
        "legacy_plaintext_email_tokens_cleared": 1,
    }
    remaining_jtis = set(
        (await maintenance_db.execute(select(RefreshSession.jti_hash))).scalars().all()
    )
    assert remaining_jtis == {"2" * 64, "3" * 64}
    remaining_pending = (
        await maintenance_db.execute(select(PendingRegistration.email))
    ).scalars().all()
    assert remaining_pending == ["live@example.test"]
    await maintenance_db.refresh(user)
    assert user.password_reset_token_hash is None
    assert user.email_verification_token_hash is None
    assert await maintenance_db.scalar(
        text("SELECT email_verification_token FROM users WHERE id = :user_id"),
        {"user_id": user.id.hex},
    ) is None


@pytest.mark.asyncio
async def test_auth_maintenance_rejects_unbounded_batch_sizes(maintenance_db: AsyncSession):
    with pytest.raises(ValueError):
        await run_auth_maintenance(maintenance_db, batch_size=0)
    with pytest.raises(ValueError):
        await run_auth_maintenance(maintenance_db, batch_size=10_001)
