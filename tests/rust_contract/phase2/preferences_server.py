"""Disposable Task 9 Python oracle; production app bytes remain unchanged."""

import argparse
import os
from contextvars import ContextVar
from datetime import datetime
from uuid import UUID

preference_id = ContextVar("preference_id")
preference_clock = ContextVar("preference_clock")


class PreferenceClock:
    @staticmethod
    def now(tz=None):
        instant = preference_clock.get()
        return instant.astimezone(tz) if tz else instant.replace(tzinfo=None)


class ControlledPreferences:
    def __init__(self, app, token):
        self.app = app
        self.token = token

    async def __call__(self, scope, receive, send):
        if scope.get("type") != "http" or scope.get("method") != "PUT":
            return await self.app(scope, receive, send)
        headers = dict(scope["headers"])
        if headers.get(b"x-disposable-token", b"").decode() != self.token:
            return await self.app(scope, receive, send)
        instant = datetime.fromisoformat(headers[b"x-test-preference-clock"].decode())
        row_id = UUID(headers[b"x-test-preference-id"].decode())
        # SQLAlchemy captures its callable default when mapping the model. Patch
        # that callable, not uuid.uuid4 globally (request IDs must stay real).
        # The compiled INSERT caches the callable. Keep one callable installed
        # and change only the ContextVar read by that callable on each request.
        marker = preference_id.set(row_id)
        clock_marker = preference_clock.set(instant)
        try:
            await self.app(scope, receive, send)
        finally:
            preference_clock.reset(clock_marker)
            preference_id.reset(marker)


def main():
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("test environment required")
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    import uvicorn

    from app.main import app
    from app.models.user_preference import UserPreference
    UserPreference.__table__.c.id.default.arg = lambda context: preference_id.get()
    from tests.disposable_server import DisposableIdentityApp, validate_config

    token = validate_config("test", os.environ["DISPOSABLE_TEST_TOKEN"], "127.0.0.1", args.port, os.environ["RELEASE_SHA"])
    from app.routers import preferences
    preferences.datetime = PreferenceClock
    application = DisposableIdentityApp(ControlledPreferences(app, token), token)
    uvicorn.run(application, host="127.0.0.1", port=args.port, lifespan="off", log_level="warning")


if __name__ == "__main__":
    main()
