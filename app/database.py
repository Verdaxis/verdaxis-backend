from sqlalchemy.ext.asyncio import create_async_engine, AsyncSession, async_sessionmaker
from sqlalchemy.orm import DeclarativeBase
from app.config import Settings, settings


def engine_options(config: Settings) -> dict:
    """Build engine options without applying PostgreSQL pooling to SQLite."""
    if config.DATABASE_URL.startswith("sqlite"):
        return {}
    return {
        "pool_size": config.DB_POOL_SIZE,
        "max_overflow": config.DB_MAX_OVERFLOW,
        "pool_timeout": config.DB_POOL_TIMEOUT,
        "pool_pre_ping": True,
        "pool_recycle": config.DB_POOL_RECYCLE,
    }


pool_kwargs = engine_options(settings)

engine = create_async_engine(
    settings.DATABASE_URL,
    echo=False,
    future=True,
    **pool_kwargs,
)

AsyncSessionLocal = async_sessionmaker(
    bind=engine,
    class_=AsyncSession,
    expire_on_commit=False,
    autoflush=False,
)

class Base(DeclarativeBase):
    pass

async def get_db():
    async with AsyncSessionLocal() as session:
        try:
            yield session
        finally:
            await session.close()
