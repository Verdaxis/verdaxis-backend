from types import SimpleNamespace

import pytest
from sqlalchemy.exc import DBAPIError

from app import database
from app.main import database_contention_handler
from app.services.db_errors import is_contention_error, is_market_path


class _PostgresError(Exception):
    def __init__(self, message: str, sqlstate: str):
        super().__init__(message)
        self.sqlstate = sqlstate


def _db_error(message: str, sqlstate: str) -> DBAPIError:
    return DBAPIError.instance(
        statement="UPDATE market_data SET value = 1",
        params=None,
        orig=_PostgresError(message, sqlstate),
        dbapi_base_err=Exception,
    )


def test_market_lock_timeout_and_deadlock_are_transient():
    assert is_contention_error(_db_error("canceling statement due to lock timeout", "55P03"))
    assert is_contention_error(_db_error("deadlock detected", "40P01"))
    assert is_contention_error(_db_error("could not serialize access", "40001"))


def test_unrelated_database_errors_are_not_mapped_as_contention():
    assert not is_contention_error(_db_error("duplicate key value", "23505"))
    assert not is_contention_error(_db_error("deadlock detected", "XX000"))
    assert is_market_path("/api/orderbook")
    assert is_market_path("/api/orderbook/asks")
    assert is_market_path("/api/trades/123/confirm")
    assert is_market_path("/api/prices")
    assert is_market_path("/api/prices/reference")
    assert not is_market_path("/api/prices-fake")
    assert not is_market_path("/api/orderbookish")
    assert not is_market_path("/api/tradesman")
    assert not is_market_path("/api/users")


class _SessionContext:
    def __init__(self):
        self.rolled_back = False
        self.closed = False

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return None

    async def rollback(self):
        self.rolled_back = True

    async def close(self):
        self.closed = True


@pytest.mark.asyncio
async def test_database_dependency_rolls_back_before_propagating_db_errors(monkeypatch):
    session = _SessionContext()
    monkeypatch.setattr(database, "AsyncSessionLocal", lambda: session)
    dependency = database.get_db()
    assert await anext(dependency) is session

    error = _db_error("duplicate key", "23505")
    with pytest.raises(DBAPIError):
        await dependency.athrow(error)

    assert session.rolled_back
    assert session.closed


@pytest.mark.asyncio
async def test_unknown_database_errors_log_traceback_and_return_sanitized_500(monkeypatch):
    calls = []
    monkeypatch.setattr(
        "app.main.logger",
        SimpleNamespace(exception=lambda *args, **kwargs: calls.append((args, kwargs))),
    )
    response = await database_contention_handler(
        SimpleNamespace(url=SimpleNamespace(path="/api/prices")),
        _db_error("sensitive database detail", "23505"),
    )

    assert response.status_code == 500
    assert b"sensitive database detail" not in response.body
    assert calls
