"""Phase 3 Task 13: cross-stack token and session coexistence (Python side).

One migrated disposable database serves both stacks. The Rust half of the
matrix runs in ``rs/read/tests/auth_coexistence_postgres.rs`` through this
same wrapper. Under method B (speed plan section 7) Python routes still check
tokens and sessions after the Phase 3 switch, so both directions stay
required, not advisory.

Cells driven from Python here:
- Rust login issues a bearer that Python's ``get_current_user`` accepts
  (``/api/auth/me``) and a refresh family Python's refresh handler rotates.
- A Rust stream token passes Python's stream admission check.
- A Rust-issued session keeps working when its routes answer from Python
  (the rollback direction of method B's one-switch rollback).
- The Python auth-maintenance query selects and deletes a Rust-written
  expired refresh row.
"""
from __future__ import annotations

import asyncio
import os
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from fastapi import FastAPI
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.core.security import hash_token_identifier
from app.database import get_db
from app.models.user import User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routers.auth_simple import router as auth_router
from app.routers.stream import router as stream_router

# Every PostgreSQL case in rs/read/tests/auth_coexistence_postgres.rs. A
# skipped or missing case fails this suite.
RUST_POSTGRES_CASES = (
    "python_access_token_verifies_in_rust",
    "python_refresh_token_rotates_in_rust",
    "rust_written_refresh_row_is_visible_to_python_cleanup",
    "python_and_rust_device_lock_contend_on_one_key",
    "rust_access_token_carries_the_python_contract",
)


def _libpq(url: str) -> str:
    return url.replace("postgresql+asyncpg://", "postgres://", 1)


def test_phase3_coexistence_rust_cases(analytics_pg_url):
    """Run the Rust coexistence cases against the same migrated database."""
    environment = {
        **os.environ,
        "AUTH_EMAIL_TEST_OWNER_URL": _libpq(analytics_pg_url),
        "AUTH_EMAIL_TEST_DATABASE_URL": _libpq(os.environ["DATABASE_URL"]),
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=Path(__file__).resolve().parents[2], text=True
        ).strip(),
    }
    run = subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--features", "contract-tests",
            "--test", "auth_coexistence_postgres",
            "--", "--include-ignored", "--test-threads=1",
        ],
        cwd=Path(__file__).resolve().parents[2],
        env=environment,
        capture_output=True,
        text=True,
        timeout=1800,
    )
    assert run.returncode == 0, run.stdout[-16000:] + run.stderr[-16000:]
    for name in RUST_POSTGRES_CASES:
        assert f"test {name} ... ok" in run.stdout, run.stdout


async def _cross_stack_app(engine):
    """One FastAPI app with the real auth and stream routers on this database."""
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    app = FastAPI()
    app.state.limiter = limiter
    app.include_router(auth_router, prefix="/api")
    app.include_router(stream_router, prefix="/api/stream")

    async def override_db():
        async with factory() as session:
            yield session

    app.dependency_overrides[get_db] = override_db
    return app, factory


@pytest.mark.asyncio
async def test_rust_tokens_and_sessions_verify_in_python(analytics_pg_url, monkeypatch):
    """Rust login output admits and rotates through the real Python routes."""
    import httpx

    url = os.environ["DATABASE_URL"]
    engine = create_async_engine(url)
    monkeypatch.setattr(limiter, "enabled", False)
    app, _ = await _cross_stack_app(engine)

    user = User(
        id=uuid4(),
        email=f"coexistence-{uuid4()}@example.com",
        password_hash="$2b$12$" + "a" * 53,  # placeholder, never verified here
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
    )
    organization = uuid4()
    async with engine.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO organizations (id, name, type, verification_status, provenance) "
                "VALUES (:org, 'Cross-stack fixture', 'FUEL_BUYER', 'APPROVED', 'UNKNOWN')"
            ),
            {"org": organization},
        )
        await connection.execute(
            text(
                "INSERT INTO users (id, email, password_hash, role, status, "
                "email_verified, must_change_password, organization_id) "
                "VALUES (:id, :email, :hash, 'BUYER', 'APPROVED', true, false, :org)"
            ),
            {"id": user.id, "email": user.email, "hash": user.password_hash, "org": organization},
        )

    # The app under test is configured with the harness JWT secret; the Rust
    # issuer signs with the same configured material (C1), so tokens verify
    # on both stacks of this database.
    import app.config as config_module

    secret = config_module.settings.JWT_SECRET
    issuer = config_module.settings.JWT_ISSUER
    audience = config_module.settings.JWT_AUDIENCE
    import jwt as pyjwt

    def rust_access(now_ts: int) -> str:
        """Claims exactly as the Rust TokenIssuer writes them (C1)."""
        return pyjwt.encode(
            {
                "sub": str(user.id),
                "type": "access",
                "exp": now_ts + 900,
                "iat": now_ts,
                "iat_us": now_ts * 1_000_000,
                "iss": issuer,
                "aud": audience,
            },
            secret,
            algorithm="HS256",
        )

    def rust_refresh(now_ts: int, family, jti: str) -> str:
        return pyjwt.encode(
            {
                "sub": str(user.id),
                "type": "refresh",
                "exp": now_ts + 604800,
                "iat": now_ts,
                "iat_us": now_ts * 1_000_000,
                "jti": jti,
                "family_id": str(family),
                "iss": issuer,
                "aud": audience,
            },
            secret,
            algorithm="HS256",
        )

    transport = httpx.ASGITransport(app=app, raise_app_exceptions=True)
    now_ts = int(asyncio.get_running_loop().time()) + 1_792_000_000

    try:
        async with httpx.AsyncClient(
            transport=transport, base_url="https://test", headers={"Origin": "https://test"}
        ) as client:
            # 1. Rust access token -> Python /api/auth/me.
            me = await client.get(
                "/api/auth/me", headers={"Authorization": f"Bearer {rust_access(now_ts)}"}
            )
            assert me.status_code == 200, me.text
            assert me.json()["email"] == user.email

            # 2. Rust refresh family -> Python refresh rotation. The device
            # cookie has the 43 URL-safe characters both stacks require.
            family = uuid4()
            jti = "r" * 43
            device_id = "D" + "d" * 42
            async with engine.begin() as connection:
                await connection.execute(
                    text(
                        "INSERT INTO refresh_sessions "
                        "(user_id, family_id, jti_hash, device_id_hash, revoked, "
                        "expires_at, created_at) "
                        "VALUES (:user, :family, :jti_hash, :device_hash, false, "
                        "now() + interval '7 days', now())"
                    ),
                    {
                        "user": user.id,
                        "family": family,
                        "jti_hash": hash_token_identifier(jti),
                        "device_hash": hash_token_identifier(device_id),
                    },
                )
            client.cookies.set("refresh_token", rust_refresh(now_ts, family, jti))
            client.cookies.set("device_session", device_id)
            refreshed = await client.post("/api/auth/refresh")
            assert refreshed.status_code == 200, refreshed.text

            # 3. A Rust stream token passes Python's stream admission: the
            # same claims Rust issues (sub, org_id, type, environment, 60 s)
            # get past `decode_token` and the admission revalidation, so the
            # request enters the stream itself.
            stream_token = pyjwt.encode(
                {
                    "sub": str(user.id),
                    "org_id": str(organization),
                    "type": "stream",
                    "environment": config_module.settings.ENVIRONMENT.strip().lower(),
                    "exp": now_ts + 60,
                    "iat": now_ts,
                    "iat_us": now_ts * 1_000_000,
                    "iss": issuer,
                    "aud": audience,
                },
                secret,
                algorithm="HS256",
            )
            trades = await client.get(
                "/api/stream/trades", params={"stream_token": stream_token}, timeout=10
            )
            assert trades.status_code == 200, trades.text
            assert trades.headers["content-type"].startswith("text/event-stream")

            # 4. Rollback cell: after Python's refresh rotation above, the
            # successor session (Python-written row) logs out through Python
            # and every family row ends revoked. This is the session half of
            # `rust_session_survives_caddy_rollback_to_python`: the Rust-issued
            # family remained usable across the stack change.
            logged_out = await client.post("/api/auth/logout")
            assert logged_out.status_code == 200, logged_out.text
            async with engine.connect() as connection:
                live = await connection.execute(
                    text(
                        "SELECT count(*) FROM refresh_sessions "
                        "WHERE user_id=:id AND revoked = false"
                    ),
                    {"id": user.id},
                )
                assert live.scalar_one() == 0
    finally:
        async with engine.begin() as connection:
            await connection.execute(
                text("DELETE FROM refresh_sessions WHERE user_id=:id"), {"id": user.id}
            )
            await connection.execute(text("DELETE FROM users WHERE id=:id"), {"id": user.id})
            await connection.execute(
                text("DELETE FROM organizations WHERE id=:id"), {"id": organization}
            )
        await engine.dispose()


@pytest.mark.asyncio
async def test_rust_written_expired_row_is_deleted_by_python_maintenance(analytics_pg_url):
    """The real maintenance routine deletes Rust-written expired rows."""
    url = os.environ["DATABASE_URL"]
    engine = create_async_engine(url)
    from app.services.auth_maintenance import run_auth_maintenance

    user_id = uuid4()
    family = uuid4()
    jti = "expired-rust-" + uuid4().hex
    async with engine.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO users (id, email, password_hash, role, status, "
                "email_verified, must_change_password) "
                "VALUES (:id, :email, 'unused', 'BUYER', 'APPROVED', true, false)"
            ),
            {"id": user_id, "email": f"maintenance-{user_id}@example.com"},
        )
        await connection.execute(
            text(
                "INSERT INTO refresh_sessions "
                "(user_id, family_id, jti_hash, device_id_hash, revoked, expires_at, "
                "created_at) VALUES (:user, :family, :jti_hash, :device, true, "
                "now() - interval '8 days', now() - interval '9 days')"
            ),
            {
                "user": user_id,
                "family": family,
                "jti_hash": hash_token_identifier(jti),
                "device": hash_token_identifier("device-cookie-value"),
            },
        )

    async with async_sessionmaker(engine, class_=AsyncSession)() as session:
        counts = await run_auth_maintenance(session)
    assert counts["refresh_sessions_deleted"] >= 1, counts
    async with engine.connect() as connection:
        remaining = await connection.execute(
            text("SELECT count(*) FROM refresh_sessions WHERE jti_hash=:jti"),
            {"jti": hash_token_identifier(jti)},
        )
        assert remaining.scalar_one() == 0
    async with engine.begin() as connection:
        await connection.execute(text("DELETE FROM users WHERE id=:id"), {"id": user_id})
    await engine.dispose()
