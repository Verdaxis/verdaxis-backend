"""Disposable alert oracle: real Python routes with controlled generated values."""
import asyncio
import contextvars
import os
from datetime import datetime
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import event
from sqlalchemy.ext.asyncio import AsyncSession
from starlette.middleware.base import BaseHTTPMiddleware
from starlette.responses import JSONResponse

CONTROL = contextvars.ContextVar("alert_control", default={})
BARRIERS = {}


def install(app):
    from app.database import AsyncSessionLocal, get_db
    from app.models.alerts import PriceAlert

    @event.listens_for(PriceAlert, "before_insert")
    def generated(_mapper, _connection, row):
        control = CONTROL.get()
        row.id = UUID(control["x-alert-id"])
        row.created_at = datetime.fromisoformat(control["x-alert-clock"])

    class Session(AsyncSession):
        async def execute(self, statement, *args, **kwargs):
            result = await super().execute(statement, *args, **kwargs)
            group = CONTROL.get().get("x-alert-barrier")
            if group and "count(price_alerts.id)" in str(statement):
                await asyncio.wait_for(BARRIERS.setdefault(group, asyncio.Barrier(2)).wait(), 10)
            return result

        async def commit(self):
            if CONTROL.get().get("x-alert-fault") == "before-commit":
                await self.flush()
                await self.rollback()
                raise HTTPException(500, "Database operation failed.")
            await super().commit()

    async def database():
        async with Session(bind=AsyncSessionLocal.kw["bind"], expire_on_commit=False) as session:
            yield session

    app.dependency_overrides[get_db] = database

    class Controls(BaseHTTPMiddleware):
        async def dispatch(self, request, call_next):
            token = CONTROL.set(dict(request.headers))
            try:
                response = await call_next(request)
                if request.headers.get("x-alert-fault") == "after-commit" and response.status_code == 201:
                    return JSONResponse({"detail": "response unavailable after commit"}, status_code=503, headers={"x-request-id": request.headers["x-request-id"]})
                return response
            finally:
                CONTROL.reset(token)

    app.add_middleware(Controls)


def main():
    if os.environ.get("ENVIRONMENT") != "test":
        raise RuntimeError("disposable only")
    from app.main import app
    from tests.rust_contract.server import ContractOracleApp
    install(app)
    import uvicorn
    bind = os.environ["PHASE2_BIND"].split(":")
    assert bind[0] == "127.0.0.1" and int(bind[1]) > 0
    uvicorn.run(ContractOracleApp(app, os.environ["DISPOSABLE_TEST_TOKEN"]), host=bind[0], port=int(bind[1]), log_level="warning", lifespan="off")


if __name__ == "__main__":
    main()
