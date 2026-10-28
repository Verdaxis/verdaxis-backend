"""Disposable watchlist oracle: the unchanged app with controlled generated values.

The runner gives both stacks the same action clock, row identifier sequence and
failpoint through request headers. This wrapper applies them to the Python
application's column defaults and one summary call. It runs only under
ENVIRONMENT=test behind the disposable-target token; production bytes are
unchanged and no header is read in a release build.
"""

import argparse
import os
from contextvars import ContextVar
from datetime import UTC, datetime
from uuid import UUID, uuid4

clock = ContextVar("watchlist_clock", default=None)
identifiers = ContextVar("watchlist_identifiers", default=None)
fail_summary = ContextVar("watchlist_fail_summary", default=False)

WATCHLIST_TABLES = ("watchlists", "watchlist_entries", "watchlist_targets", "watchlist_events")


def next_identifier(_context=None):
    queue = identifiers.get()
    if queue is None:
        return uuid4()
    if not queue:
        raise AssertionError("controlled identifier sequence exhausted")
    return queue.pop(0)


def current_time(_context=None):
    instant = clock.get()
    return instant if instant is not None else datetime.now(UTC)


class ControlledWatchlists:
    def __init__(self, app, token):
        self.app = app
        self.token = token

    async def __call__(self, scope, receive, send):
        if scope.get("type") != "http":
            return await self.app(scope, receive, send)
        headers = dict(scope["headers"])
        controlled = (
            headers.get(b"x-disposable-token", b"").decode() == self.token
            and b"x-test-watchlist-clock" in headers
        )
        if not controlled:
            return await self.app(scope, receive, send)
        ids = [UUID(part) for part in headers.get(b"x-test-watchlist-ids", b"").decode().split(",") if part]
        markers = (
            clock.set(datetime.fromisoformat(headers[b"x-test-watchlist-clock"].decode())),
            identifiers.set(ids),
            fail_summary.set(headers.get(b"x-test-watchlist-fail-summary") == b"true"),
        )
        try:
            await self.app(scope, receive, send)
        finally:
            for variable, marker in zip((clock, identifiers, fail_summary), markers):
                variable.reset(marker)


def install_controls():
    """Patch column defaults once; ContextVars choose the value per request."""
    from app.models.watchlist import (
        Watchlist,
        WatchlistEntry,
        WatchlistEvent,
        WatchlistTarget,
    )
    from app.routers import watchlists as router
    from sqlalchemy.exc import OperationalError

    for model in (Watchlist, WatchlistEntry, WatchlistTarget, WatchlistEvent):
        table = model.__table__
        assert table.name in WATCHLIST_TABLES
        table.c.id.default.arg = next_identifier
        table.c.created_at.default.arg = current_time

    original = router.build_watchlist_summary

    async def summary_with_failpoint(db, watchlist):
        if fail_summary.get():
            # Same class of failure the application's handler turns into its 500.
            raise OperationalError("SELECT disposable", {}, Exception("disposable summary failure"))
        return await original(db, watchlist)

    router.build_watchlist_summary = summary_with_failpoint


def main():
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("test environment required")
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    import uvicorn
    from app.main import app
    from tests.disposable_server import DisposableIdentityApp, validate_config

    install_controls()
    token = validate_config("test", os.environ["DISPOSABLE_TEST_TOKEN"], "127.0.0.1", args.port,
                            os.environ["RELEASE_SHA"])
    application = DisposableIdentityApp(ControlledWatchlists(app, token), token)
    uvicorn.run(application, host="127.0.0.1", port=args.port, lifespan="off", log_level="warning")


if __name__ == "__main__":
    main()
