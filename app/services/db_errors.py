"""Bounded API responses for transient market database contention."""

from sqlalchemy.exc import DBAPIError


MARKET_PATH_ROOTS = (
    "/api/orderbook",
    "/api/trades",
    "/api/prices",
    "/api/price-discovery",
    "/api/matchmaking",
)
_TRANSIENT_LOCK_STATES = {"55P03", "40P01", "40001"}


def is_contention_error(exc: DBAPIError) -> bool:
    """Recognize PostgreSQL lock/serialization failures without exposing SQL."""
    original = getattr(exc, "orig", exc)
    sqlstate = (
        getattr(original, "sqlstate", None)
        or getattr(original, "pgcode", None)
        or getattr(original, "sqlstate_code", None)
    )
    return sqlstate in _TRANSIENT_LOCK_STATES


def is_market_path(path: str) -> bool:
    return any(path == root or path.startswith(f"{root}/") for root in MARKET_PATH_ROOTS)
