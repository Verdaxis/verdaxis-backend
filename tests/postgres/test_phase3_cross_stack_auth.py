"""Real Python and release-profile Rust handlers on one disposable database."""
from __future__ import annotations

import os
import socket
import subprocess
from contextlib import asynccontextmanager
from pathlib import Path
from uuid import uuid4

import httpx
import pytest
from fastapi import FastAPI
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.config import settings
from app.core.security import get_password_hash
from app.database import get_db
from app.rate_limit import limiter
from app.routers.auth_simple import router as auth_router

ROOT = Path(__file__).resolve().parents[2]
ORIGIN = "https://app.verdaxis.exchange"
PASSWORD = "CoexistencePassword9"


def cargo(*arguments):
    """Respect the shared target lock, including cargo invoked by pytest."""
    home = Path(os.environ.get("VERDAXIS_TEST_HOME", "/home/verdaxis-test"))
    return ["flock", "-w", "7200", str(home / "runs/.lock-target-phase3"), "cargo", *arguments]


@pytest.fixture(scope="module")
def rust_auth_server(analytics_pg_url):
    env = {
        **os.environ,
        "SQLX_OFFLINE": "true",
        "RELEASE_SHA": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "PHASE3_DATABASE_URL": os.environ["DATABASE_URL"].replace("postgresql+asyncpg://", "postgres://", 1),
        "JWT_ISSUER": settings.JWT_ISSUER,
        "JWT_AUDIENCE": settings.JWT_AUDIENCE,
        "JWT_SECRET": settings.JWT_SECRET,
        "ENVIRONMENT": "test",
    }
    build = subprocess.run(
        cargo("build", "--manifest-path", "rs/Cargo.toml", "--locked", "--release",
              "-p", "verdaxis-read", "--features", "contract-tests", "--example", "phase3_auth"),
        cwd=ROOT, env=env, capture_output=True, text=True, timeout=1800,
    )
    assert build.returncode == 0, build.stderr[-12000:]
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    env["PHASE3_BIND"] = f"127.0.0.1:{port}"
    target = Path(env.get("CARGO_TARGET_DIR", ROOT / "rs/target"))
    # stdout/stderr are not captured: handlers must never print credentials.
    process = subprocess.Popen([str(target / "release/examples/phase3_auth")], cwd=ROOT, env=env,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        import time
        deadline = time.monotonic() + 30
        while True:
            assert process.poll() is None, "Rust disposable adapter exited during startup"
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                    break
            except OSError:
                assert time.monotonic() < deadline, "Rust disposable adapter did not start"
                time.sleep(0.05)
        yield f"http://127.0.0.1:{port}"
    finally:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=10)


@asynccontextmanager
async def account(analytics_pg_url):
    owner = create_async_engine(analytics_pg_url)
    runtime = create_async_engine(os.environ["DATABASE_URL"])
    user, organization = uuid4(), uuid4()
    email = f"coexistence-{user}@example.com"
    async with owner.begin() as conn:
        await conn.execute(text("INSERT INTO organizations (id,name,type,verification_status,provenance) "
                                "VALUES (:id,'Cross-stack fixture','FUEL_BUYER','APPROVED','UNKNOWN')"),
                           {"id": organization})
        await conn.execute(text("INSERT INTO users (id,email,password_hash,role,status,email_verified,"
                                "must_change_password,organization_id) VALUES "
                                "(:id,:email,:password,'BUYER','APPROVED',true,false,:org)"),
                           {"id": user, "email": email, "password": get_password_hash(PASSWORD), "org": organization})
    try:
        yield owner, runtime, user, organization, email
    finally:
        async with owner.begin() as conn:
            for table in ("user_login_days", "refresh_sessions"):
                await conn.execute(text(f"DELETE FROM {table} WHERE user_id=:id"), {"id": user})
            await conn.execute(text("DELETE FROM users WHERE id=:id"), {"id": user})
            await conn.execute(text("DELETE FROM organizations WHERE id=:id"), {"id": organization})
        await runtime.dispose()
        await owner.dispose()


def python_app(engine):
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    app = FastAPI()
    app.state.limiter = limiter
    app.include_router(auth_router, prefix="/api")

    async def database():
        async with factory() as session:
            yield session

    app.dependency_overrides[get_db] = database
    return app, factory


def copy_cookies(source, destination):
    for cookie in source.cookies.jar:
        destination.cookies.set(cookie.name, cookie.value, path="/api/auth")


@pytest.mark.asyncio
async def test_real_handlers_accept_each_others_tokens_and_sessions(
    analytics_pg_url, rust_auth_server, monkeypatch
):
    """Both login implementations issue the actual tokens the other stack consumes."""
    from starlette.requests import Request

    from app.routers import stream

    monkeypatch.setattr(limiter, "enabled", False)
    async with account(analytics_pg_url) as (owner, runtime, user, organization, email):
        app, factory = python_app(runtime)
        monkeypatch.setattr(stream, "AsyncSessionLocal", factory)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url="https://test",
                                     headers={"Origin": ORIGIN}) as python, \
                   httpx.AsyncClient(base_url=rust_auth_server, headers={"Origin": ORIGIN}) as rust:
            for producer, consumer in ((python, rust), (rust, python)):
                producer.cookies.clear()
                consumer.cookies.clear()
                login = await producer.post("/api/auth/login", data={"username": email, "password": PASSWORD})
                assert login.status_code == 200, f"login status {login.status_code}"
                bearer = login.json()["access_token"]
                read_path = "/api/notifications" if consumer is rust else "/api/auth/me"
                read = await consumer.get(read_path, headers={"Authorization": f"Bearer {bearer}"})
                assert read.status_code == 200, f"cross-stack bearer status {read.status_code}"
                copy_cookies(producer, consumer)
                rotated = await consumer.post("/api/auth/refresh")
                assert rotated.status_code == 200, f"cross-stack refresh status {rotated.status_code}"
                async with owner.connect() as conn:
                    rows = (await conn.execute(text("SELECT revoked,replaced_by_jti_hash FROM refresh_sessions "
                                                     "WHERE user_id=:id ORDER BY created_at"), {"id": user})).all()
                assert sum(not row.revoked for row in rows) == 1
                assert any(row.replaced_by_jti_hash is not None for row in rows)
                # A Rust-issued stream token is checked by Python's real admission
                # handler, without consuming an infinite SSE response body.
                if producer is rust:
                    issued = await rust.get("/api/auth/stream-token", headers={"Authorization": f"Bearer {bearer}"})
                    assert issued.status_code == 200
                    response = await stream.stream_trades(
                        Request({"type": "http", "method": "GET", "path": "/api/stream/trades", "headers": []}),
                        stream_token=issued.json()["stream_token"], last_event_id=None,
                    )
                    assert response.status_code == 200
                    assert response.media_type == "text/event-stream"
                logout = await consumer.post("/api/auth/logout")
                assert logout.status_code in (200, 204)
                async with owner.connect() as conn:
                    assert (await conn.execute(text("SELECT count(*) FROM refresh_sessions WHERE user_id=:id "
                                                    "AND NOT revoked"), {"id": user})).scalar_one() == 0
                async with owner.begin() as conn:
                    await conn.execute(text("DELETE FROM refresh_sessions WHERE user_id=:id"), {"id": user})


@pytest.mark.asyncio
async def test_python_maintenance_deletes_a_rust_written_row(analytics_pg_url, rust_auth_server, monkeypatch):
    from app.services.auth_maintenance import run_auth_maintenance

    monkeypatch.setattr(limiter, "enabled", False)
    async with account(analytics_pg_url) as (owner, runtime, user, _, email):
        async with httpx.AsyncClient(base_url=rust_auth_server, headers={"Origin": ORIGIN}) as rust:
            login = await rust.post("/api/auth/login", data={"username": email, "password": PASSWORD})
            assert login.status_code == 200
        async with owner.begin() as conn:
            await conn.execute(text("UPDATE refresh_sessions SET expires_at=now()-interval '1 day' WHERE user_id=:id"),
                               {"id": user})
        async with async_sessionmaker(runtime)() as session:
            counts = await run_auth_maintenance(session)
            await session.commit()
        assert counts["refresh_sessions_deleted"] >= 1
        async with owner.connect() as conn:
            assert (await conn.execute(text("SELECT count(*) FROM refresh_sessions WHERE user_id=:id"),
                                       {"id": user})).scalar_one() == 0
