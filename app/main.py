import asyncio
import os
import time
import uuid as _uuid
from contextvars import ContextVar

from fastapi import FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse
from slowapi.errors import RateLimitExceeded

from app.config import settings
from app.rate_limit import limiter
from app.routers.auth_simple import router as auth_router
from app.admin import setup_admin

from app.routers.ports import router as ports_router
from app.routers.vessels import router as vessels_router
from app.routers.inventory import router as inventory_router
from app.routers.compliance import router as compliance_router
from app.routers.ai import router as ai_router
from app.routers.orders import router as orders_router
from app.routers.notifications import router as notifications_router
from app.routers.orderbook import router as orderbook_router
from app.routers.trades import router as trades_router
from app.routers.price_discovery import router as price_discovery_router
from app.routers.matchmaking import router as matchmaking_router
from app.routers.producers import router as producers_router
from app.routers.availability import router as availability_router
from app.routers.demand import router as demand_router
from app.routers.fleet_intelligence import router as fleet_intel_router
from app.routers.audit import router as audit_router
from app.routers.stream import router as stream_router
from app.routers.compliance_api import router as compliance_api_router
from app.routers.admin_analytics import router as admin_analytics_router
from app.routers.kyc import router as kyc_router
from app.routers.catalog import router as catalog_router
from app.routers.curves import router as curves_router
from app.routers.alerts import router as alerts_router
from app.routers.activity import router as activity_router
from app.routers.subscriptions import router as subscriptions_router
from app.routers.referrals import router as referrals_router
from app.routers.trade_tape import router as trade_tape_router
from app.routers.rfq import router as rfq_router
from app.routers.negotiations import router as negotiations_router
from app.routers.watchlists import router as watchlists_router
from app.routers.news import router as news_router

# ---------------------------------------------------------------------------
# Structured logging
# ---------------------------------------------------------------------------
import structlog

structlog.configure(
    processors=[
        structlog.contextvars.merge_contextvars,
        structlog.processors.add_log_level,
        structlog.processors.TimeStamper(fmt="iso"),
        structlog.processors.StackInfoRenderer(),
        structlog.processors.format_exc_info,
        structlog.processors.JSONRenderer(),
    ],
    wrapper_class=structlog.make_filtering_bound_logger(0),
    context_class=dict,
    logger_factory=structlog.PrintLoggerFactory(),
    cache_logger_on_first_use=True,
)

logger = structlog.get_logger()

# Request correlation ID context var
request_id_ctx: ContextVar[str] = ContextVar("request_id", default="")

# SECURITY: Disable /docs and /redoc in production to prevent API schema exposure
_docs_url = "/docs" if os.getenv("ENVIRONMENT") != "production" else None
_redoc_url = "/redoc" if os.getenv("ENVIRONMENT") != "production" else None

# ---------------------------------------------------------------------------
# Lifespan: background news feed refresh every 15 minutes
# ---------------------------------------------------------------------------
from contextlib import asynccontextmanager

@asynccontextmanager
async def lifespan(app: FastAPI):
    async def _news_refresh_loop():
        from app.database import AsyncSessionLocal
        from app.services.news_feed import refresh_news
        while True:
            try:
                async with AsyncSessionLocal() as db:
                    await refresh_news(db)
            except Exception:
                logger.warning("news_refresh_loop.error", exc_info=True)
            await asyncio.sleep(900)  # 15 minutes

    task = asyncio.create_task(_news_refresh_loop())
    yield
    task.cancel()

app = FastAPI(
    title="Verdaxis Intelligence Cockpit",
    description="Maritime intelligence and procurement platform backend",
    version="1.0.0",
    docs_url=_docs_url,
    redoc_url=_redoc_url,
    lifespan=lifespan,
)

# ---------------------------------------------------------------------------
# Rate limiter
# ---------------------------------------------------------------------------
app.state.limiter = limiter

@app.exception_handler(RateLimitExceeded)
async def rate_limit_handler(request: Request, exc: RateLimitExceeded):
    return JSONResponse(
        status_code=429,
        content={"detail": f"Rate limit exceeded: {exc.detail}"},
    )

# Admin panel
setup_admin(app)

# CORS
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.BACKEND_CORS_ORIGINS,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

# ---------------------------------------------------------------------------
# Request logging middleware (structlog + correlation IDs)
# ---------------------------------------------------------------------------
@app.middleware("http")
async def request_logging_middleware(request: Request, call_next):
    rid = request.headers.get("X-Request-ID") or str(_uuid.uuid4())
    request_id_ctx.set(rid)
    structlog.contextvars.clear_contextvars()
    structlog.contextvars.bind_contextvars(request_id=rid)

    start = time.perf_counter()
    response = await call_next(request)
    duration_ms = round((time.perf_counter() - start) * 1000, 1)

    logger.info(
        "http_request",
        method=request.method,
        path=request.url.path,
        status=response.status_code,
        duration_ms=duration_ms,
        client=request.client.host if request.client else None,
    )

    response.headers["X-Request-ID"] = rid
    return response


# ---------------------------------------------------------------------------
# Routers
# ---------------------------------------------------------------------------
app.include_router(auth_router, prefix=settings.API_V1_STR)
app.include_router(ports_router, prefix=settings.API_V1_STR)
app.include_router(vessels_router, prefix=settings.API_V1_STR)
app.include_router(inventory_router, prefix=settings.API_V1_STR)
app.include_router(compliance_router, prefix=settings.API_V1_STR)
app.include_router(ai_router, prefix=settings.API_V1_STR)
app.include_router(orders_router, prefix=settings.API_V1_STR)
app.include_router(notifications_router, prefix=settings.API_V1_STR)
app.include_router(orderbook_router, prefix=settings.API_V1_STR)
app.include_router(trades_router, prefix=settings.API_V1_STR)
app.include_router(price_discovery_router, prefix=settings.API_V1_STR)
app.include_router(matchmaking_router, prefix=settings.API_V1_STR)
app.include_router(producers_router, prefix=settings.API_V1_STR)
app.include_router(availability_router, prefix=settings.API_V1_STR)
app.include_router(demand_router, prefix=settings.API_V1_STR)
app.include_router(audit_router, prefix=settings.API_V1_STR)
app.include_router(stream_router, prefix=settings.API_V1_STR)
app.include_router(compliance_api_router, prefix=settings.API_V1_STR)
app.include_router(admin_analytics_router, prefix=settings.API_V1_STR)
app.include_router(kyc_router, prefix=settings.API_V1_STR)
app.include_router(catalog_router, prefix=settings.API_V1_STR)
app.include_router(curves_router, prefix=settings.API_V1_STR)
app.include_router(alerts_router, prefix=settings.API_V1_STR)
app.include_router(activity_router, prefix=settings.API_V1_STR)
app.include_router(subscriptions_router, prefix=settings.API_V1_STR)
app.include_router(referrals_router, prefix=settings.API_V1_STR)
app.include_router(trade_tape_router, prefix=settings.API_V1_STR)
app.include_router(watchlists_router, prefix=settings.API_V1_STR)
app.include_router(rfq_router, prefix=settings.API_V1_STR)
app.include_router(negotiations_router, prefix=settings.API_V1_STR)
app.include_router(news_router, prefix=settings.API_V1_STR)
app.include_router(fleet_intel_router, prefix=settings.API_V1_STR)

from app.routers import dashboard
app.include_router(dashboard.router, prefix=settings.API_V1_STR)


@app.get("/")
async def root():
    return {"message": "Verdaxis API is running", "version": "1.0.0"}

@app.get("/health")
async def health_check():
    return {"status": "ok"}

@app.get("/health/live")
async def health_live():
    """Minimal liveness probe — is the process up?"""
    return {"status": "ok"}

@app.get("/health/ready")
async def health_ready():
    """Readiness probe — checks DB connectivity."""
    from app.database import engine
    from sqlalchemy import text
    try:
        async with engine.connect() as conn:
            await conn.execute(text("SELECT 1"))
        return {"status": "ok", "db": "connected"}
    except Exception as e:
        from fastapi.responses import JSONResponse
        return JSONResponse(
            status_code=503,
            content={"status": "error", "db": str(e)},
        )
