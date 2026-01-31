from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from app.config import settings
from app.routers.auth_simple import router as auth_router

# ... previous imports ...

# app.include_router(auth_router, prefix=settings.API_V1_STR) # Replaced
app.include_router(auth_router, prefix=settings.API_V1_STR)
from app.routers.ports import router as ports_router
from app.routers.vessels import router as vessels_router
from app.routers.quotes import router as quotes_router
from app.routers.inventory import router as inventory_router
from app.routers.compliance import router as compliance_router
from app.routers.ai import router as ai_router
from app.routers.listings import router as listings_router
from app.routers.rfq import router as rfq_router

app = FastAPI(
    title="Verdaxis Intelligence Cockpit",
    description="Maritime intelligence and procurement platform backend",
    version="1.0.0",
)

# CORS Configuration
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.BACKEND_CORS_ORIGINS,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(auth_router, prefix=settings.API_V1_STR)
app.include_router(ports_router, prefix=settings.API_V1_STR)
app.include_router(vessels_router, prefix=settings.API_V1_STR)
app.include_router(quotes_router, prefix=settings.API_V1_STR)
app.include_router(inventory_router, prefix=settings.API_V1_STR)
app.include_router(compliance_router, prefix=settings.API_V1_STR)
app.include_router(ai_router, prefix=settings.API_V1_STR)
app.include_router(listings_router, prefix=settings.API_V1_STR)
app.include_router(rfq_router, prefix=settings.API_V1_STR)

@app.get("/")

async def root():
    return {"message": "Verdaxis API is running", "version": "1.0.0"}

@app.get("/health")
async def health_check():
    return {"status": "ok"}
