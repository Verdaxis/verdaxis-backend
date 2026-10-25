"""Task 10 test-only clock/UUID and postcommit response-loss adapter."""
import contextvars
import os
import uuid
from datetime import datetime

_clock = contextvars.ContextVar("feedback_clock")
_id = contextvars.ContextVar("feedback_id")


def main():
    if os.environ.get("ENVIRONMENT") != "test":
        raise SystemExit("test environment required")
    from starlette.responses import JSONResponse

    from app.main import app
    from app.routers import feedback

    class Clock(datetime):
        @classmethod
        def now(cls, tz=None):
            return datetime.fromisoformat(_clock.get()).astimezone(tz)

    class IDs:
        @staticmethod
        def uuid4():
            return uuid.UUID(_id.get())

    feedback.datetime = Clock
    feedback.uuid = IDs

    @app.middleware("http")
    async def control(request, call_next):
        if "x-feedback-clock" not in request.headers:
            return await call_next(request)
        clock = _clock.set(request.headers["x-feedback-clock"])
        identifier = _id.set(request.headers["x-feedback-id"])
        try:
            response = await call_next(request)
            if "x-feedback-response-loss" in request.headers and response.status_code < 300:
                return JSONResponse({"detail": "response lost after commit"}, status_code=500, headers={"x-request-id": request.headers["x-request-id"]})
            return response
        finally:
            _clock.reset(clock)
            _id.reset(identifier)

    from scripts.rust_contract.phase2_oracle import main as serve
    return serve()


if __name__ == "__main__":
    raise SystemExit(main())
