from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import Request
from sqlalchemy.dialects import postgresql

from app.core.security import create_access_token
from app.models.user import UserStatus
from app.routers import auth_simple
from app.routers.auth_simple import _resolve_authenticated_user


def _request(
    method: str,
    path: str,
    route_path: str | None = None,
    *,
    market_support_context: bool = False,
    device_id: str | None = None,
) -> Request:
    scope = {
        "type": "http",
        "method": method,
        "path": path,
        "headers": [
            *(
                [(b"x-verdaxis-market-support-context", str(uuid4()).encode())]
                if market_support_context
                else []
            ),
            *([(b"cookie", f"device_session={device_id}".encode())] if device_id else []),
        ],
        "query_string": b"",
        "scheme": "https",
        "server": ("test", 443),
        "client": ("test", 1234),
    }
    if route_path is not None:
        scope["route"] = SimpleNamespace(path=route_path)
    return Request(scope)


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("method", "path", "route_path", "market_support_context", "expected_lock"),
    (
        ("GET", "/api/auth/me", None, False, "FOR SHARE"),
        (
            "GET",
            "/api/watchlists/a550c69b-2934-4b36-87f5-e4f9976d1af0/events",
            "/api/watchlists/{watchlist_id}/events",
            False,
            "FOR SHARE",
        ),
        (
            "GET",
            "/api/trades/my",
            "/api/trades/my",
            True,
            "FOR UPDATE",
        ),
        ("PUT", "/api/auth/me/password", None, False, "FOR UPDATE"),
        ("GET", "/api/subscriptions/me", None, False, "FOR UPDATE"),
        ("GET", "/api/future-read", None, False, "FOR UPDATE"),
    ),
)
async def test_authenticated_user_uses_the_required_postgres_lock(
    method: str,
    path: str,
    route_path: str | None,
    market_support_context: bool,
    expected_lock: str,
):
    user_id = uuid4()
    user = SimpleNamespace(id=user_id, status=UserStatus.APPROVED)
    result = MagicMock()
    result.scalar_one_or_none.return_value = user
    db = AsyncMock()
    db.execute.return_value = result

    await _resolve_authenticated_user(
        _request(
            method,
            path,
            route_path,
            market_support_context=market_support_context,
        ),
        create_access_token(str(user_id)),
        db,
    )

    statement = db.execute.await_args.args[0]
    sql = str(statement.compile(dialect=postgresql.dialect()))
    assert expected_lock in sql

@pytest.mark.asyncio
async def test_password_change_locks_existing_device_before_user(monkeypatch):
    user_id = uuid4()
    user = SimpleNamespace(id=user_id, status=UserStatus.APPROVED)
    result = MagicMock()
    result.scalar_one_or_none.return_value = user
    call_order: list[str] = []
    db = AsyncMock()

    async def execute(statement):
        call_order.append("user")
        return result

    async def acquire_device_lock(session, device_id_hash):
        assert session is db
        assert device_id_hash == auth_simple._device_session_hash("a" * 43)
        call_order.append("device")

    db.execute.side_effect = execute
    monkeypatch.setattr(
        auth_simple,
        "_acquire_device_session_lock",
        acquire_device_lock,
    )

    await _resolve_authenticated_user(
        _request("PUT", "/api/auth/me/password", device_id="a" * 43),
        create_access_token(str(user_id)),
        db,
    )

    assert call_order == ["device", "user"]
