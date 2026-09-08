from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import Request
from sqlalchemy.dialects import postgresql

from app.core.security import create_access_token
from app.models.user import UserStatus
from app.routers.auth_simple import _resolve_authenticated_user


def _request(
    method: str,
    path: str,
    route_path: str | None = None,
    *,
    market_support_context: bool = False,
) -> Request:
    scope = {
        "type": "http",
        "method": method,
        "path": path,
        "headers": (
            [(b"x-verdaxis-market-support-context", str(uuid4()).encode())]
            if market_support_context
            else []
        ),
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
        ("POST", "/api/auth/me/password", None, False, "FOR UPDATE"),
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
