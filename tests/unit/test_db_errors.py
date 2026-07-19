from sqlalchemy.exc import DBAPIError

from app.services.db_errors import is_lock_timeout_or_deadlock, is_market_path


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
    assert is_lock_timeout_or_deadlock(_db_error("canceling statement due to lock timeout", "55P03"))
    assert is_lock_timeout_or_deadlock(_db_error("deadlock detected", "40P01"))
    assert is_lock_timeout_or_deadlock(_db_error("could not serialize access", "40001"))


def test_unrelated_database_errors_are_not_mapped_as_contention():
    assert not is_lock_timeout_or_deadlock(_db_error("duplicate key value", "23505"))
    assert is_market_path("/api/orderbook")
    assert is_market_path("/api/trades/123/confirm")
    assert not is_market_path("/api/users")
