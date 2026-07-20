"""Small, bounded retries at market API transaction boundaries."""
from __future__ import annotations

from functools import wraps
from inspect import signature
from typing import Any, Awaitable, Callable, TypeVar

from fastapi import HTTPException
from sqlalchemy import inspect as sqlalchemy_inspect
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import AsyncSession


RETRYABLE_MARKET_SQLSTATES = frozenset({"40001", "40P01"})
DEFAULT_MARKET_TRANSACTION_ATTEMPTS = 2


async def transaction_boundary_hook(
    boundary: str,
    *,
    operation: str,
    aggregate_id: object | None = None,
) -> None:
    """No-op transaction checkpoint that deterministic tests may replace.

    The hook is called only before acquiring a mutation lock, after the route
    has completed its read-only preview. Production never installs a callback;
    PostgreSQL locks remain the synchronization mechanism.
    """
    return None


class RetryableMarketTransactionError(RuntimeError):
    """A genuine serialization/deadlock failure safe to rerun in full."""

    def __init__(self, sqlstate: str):
        super().__init__(sqlstate)
        self.sqlstate = sqlstate


def database_sqlstate(error: BaseException) -> str | None:
    original = getattr(error, "orig", error)
    return getattr(original, "sqlstate", None) or getattr(original, "pgcode", None)


def is_retryable_market_transaction_error(error: BaseException) -> bool:
    if isinstance(error, RetryableMarketTransactionError):
        return error.sqlstate in RETRYABLE_MARKET_SQLSTATES
    return database_sqlstate(error) in RETRYABLE_MARKET_SQLSTATES


Result = TypeVar("Result")


def _bound_argument(
    function: Callable[..., Awaitable[Any]],
    args: tuple[Any, ...],
    kwargs: dict[str, Any],
    name: str,
) -> Any:
    try:
        return signature(function).bind_partial(*args, **kwargs).arguments.get(name)
    except TypeError:
        return kwargs.get(name)


def retry_market_transaction(
    *,
    max_attempts: int = DEFAULT_MARKET_TRANSACTION_ATTEMPTS,
) -> Callable[[Callable[..., Awaitable[Result]]], Callable[..., Awaitable[Result]]]:
    """Retry a complete route operation only for SQLSTATE 40001/40P01.

    A rollback starts the next attempt in a fresh transaction on the same
    request-scoped session. Lock/statement timeouts and application conflicts
    are intentionally not retried here.
    """
    if max_attempts < 1 or max_attempts > 3:
        raise ValueError("market transaction attempts must be between 1 and 3")

    def decorate(
        function: Callable[..., Awaitable[Result]],
    ) -> Callable[..., Awaitable[Result]]:
        @wraps(function)
        async def wrapped(*args: Any, **kwargs: Any) -> Result:
            db = _bound_argument(function, args, kwargs, "db")
            if not isinstance(db, AsyncSession) and not hasattr(db, "rollback"):
                return await function(*args, **kwargs)

            for attempt in range(max_attempts):
                try:
                    return await function(*args, **kwargs)
                except (RetryableMarketTransactionError, DBAPIError) as error:
                    if not is_retryable_market_transaction_error(error):
                        raise
                    await db.rollback()
                    if attempt + 1 >= max_attempts:
                        raise HTTPException(
                            status_code=503,
                            detail=(
                                "Market transaction serialization failed; "
                                "retry the request"
                            ),
                        ) from error

                    # SQLAlchemy expires persistent instances on rollback. The
                    # auth dependency's User is reused by the wrapped route, so
                    # refresh it before the next attempt when it is session-bound.
                    current_user = _bound_argument(
                        function, args, kwargs, "current_user"
                    )
                    state = sqlalchemy_inspect(current_user, raiseerr=False)
                    if state is not None and state.session is not None:
                        await db.refresh(current_user)

            raise AssertionError("unreachable market retry state")

        return wrapped

    return decorate
