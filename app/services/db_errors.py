"""Bounded API responses for transient market database contention."""

from sqlalchemy.exc import DBAPIError


MARKET_PATH_PREFIXES = (
    "/api/orderbook",
    "/api/trades",
    "/api/price-discovery",
    "/api/matchmaking",
)
_TRANSIENT_LOCK_STATES = {"55P03", "40P01", "40001"}


def is_lock_timeout_or_deadlock(exc: DBAPIError) -> bool:
    """Recognize PostgreSQL lock/serialization failures without exposing SQL."""
    original = getattr(exc, "orig", exc)
    sqlstate = (
        getattr(original, "sqlstate", None)
        or getattr(original, "pgcode", None)
        or getattr(original, "sqlstate_code", None)
    )
    message = str(original).lower()
    return sqlstate in _TRANSIENT_LOCK_STATES or any(
        marker in message
        for marker in (
            "lock timeout",
            "deadlock detected",
            "could not serialize access",
        )
    )


def is_market_path(path: str) -> bool:
    return path.startswith(MARKET_PATH_PREFIXES)
