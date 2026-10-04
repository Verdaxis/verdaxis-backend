"""Bounded API responses for transient market database contention."""

from sqlalchemy.exc import DBAPIError


MARKET_PATH_ROOTS = (
    "/api/orderbook",
    "/api/trades",
    "/api/prices",
    "/api/price-discovery",
    "/api/matchmaking",
    "/api/curves/forward",
)
_TRANSIENT_LOCK_STATES = {"55P03", "40P01", "40001"}


def database_sqlstate(error: BaseException) -> str | None:
    original = getattr(error, "orig", error)
    return (
        getattr(original, "sqlstate", None)
        or getattr(original, "pgcode", None)
        or getattr(original, "sqlstate_code", None)
    )


def _original_and_sqlstate(exc: DBAPIError) -> tuple[object, str | None]:
    original = getattr(exc, "orig", exc)
    return original, database_sqlstate(exc)


def is_contention_error(exc: DBAPIError) -> bool:
    """Recognize PostgreSQL lock/serialization failures without exposing SQL."""
    _, sqlstate = _original_and_sqlstate(exc)
    return sqlstate in _TRANSIENT_LOCK_STATES


def is_query_canceled_error(exc: DBAPIError) -> bool:
    """Recognize PostgreSQL query cancellation without broadening retries."""
    _, sqlstate = _original_and_sqlstate(exc)
    return sqlstate == "57014"


def database_error_log_fields(
    exc: DBAPIError,
    *,
    request_id: str,
    route: str,
) -> dict[str, str | None]:
    """Return only non-sensitive database failure metadata safe for logs."""
    original, sqlstate = _original_and_sqlstate(exc)
    return {
        "error_class": type(original).__name__,
        "sqlstate": sqlstate,
        "request_id": request_id,
        "route": route,
    }


def is_market_path(path: str) -> bool:
    return any(path == root or path.startswith(f"{root}/") for root in MARKET_PATH_ROOTS)
