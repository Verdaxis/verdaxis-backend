from sqlalchemy import text
from sqlalchemy.engine import make_url
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
        "connect_args": {
            "server_settings": {
                "statement_timeout": str(config.DB_STATEMENT_TIMEOUT_MS),
                "lock_timeout": str(config.DB_LOCK_TIMEOUT_MS),
                "idle_in_transaction_session_timeout": str(
                    config.DB_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS
                ),
            }
        },
    }


def migrator_connect_args(config: Settings) -> dict:
    """Return the longer-lived, still-bounded Alembic session policy."""
    return {
        "server_settings": {
            "statement_timeout": str(config.MIGRATOR_STATEMENT_TIMEOUT_MS),
            "lock_timeout": str(config.MIGRATOR_LOCK_TIMEOUT_MS),
            "idle_in_transaction_session_timeout": str(
                config.MIGRATOR_IDLE_IN_TRANSACTION_SESSION_TIMEOUT_MS
            ),
        }
    }


def configured_connection_total(config: Settings) -> int:
    return (
        config.DB_SERVICE_COUNT
        * config.UVICORN_WORKERS
        * (config.DB_POOL_SIZE + config.DB_MAX_OVERFLOW)
        + config.DB_RESERVED_CONNECTIONS
    )


def assert_database_runtime_is_safe(
    config: Settings,
    *,
    connected_user: str,
    connected_role_is_superuser: bool,
    observed_max_connections: int,
) -> None:
    """Attest the connected role and server capacity before serving traffic."""
    expected_user = make_url(config.DATABASE_URL).username or ""
    if connected_user != expected_user:
        raise RuntimeError(
            "connected current_user does not match the effective DATABASE_URL username"
        )
    if connected_role_is_superuser:
        raise RuntimeError("application database role must not be a superuser")
    required = configured_connection_total(config)
    if required > observed_max_connections:
        raise RuntimeError(
            f"configured aggregate {required} exceeds PostgreSQL max_connections "
            f"{observed_max_connections}"
        )


def assert_migrator_connection_is_safe(
    config: Settings,
    *,
    connected_user: str,
    connected_role_is_superuser: bool,
) -> None:
    expected_url = config.MIGRATOR_DATABASE_URL or config.DATABASE_URL
    expected_user = make_url(expected_url).username or ""
    if connected_user != expected_user:
        raise RuntimeError(
            "connected migrator current_user does not match the effective migration URL username"
        )
    if connected_role_is_superuser:
        raise RuntimeError("migrator database role must not be a superuser")


def verify_migrator_connection(connection, config: Settings = settings) -> None:
    expected_url = config.MIGRATOR_DATABASE_URL or config.DATABASE_URL
    if expected_url.startswith("sqlite"):
        return
    row = connection.execute(
        text(
            "SELECT current_user, rol.rolsuper FROM pg_catalog.pg_roles AS rol "
            "WHERE rol.rolname = current_user"
        )
    ).one()
    assert_migrator_connection_is_safe(
        config,
        connected_user=row[0],
        connected_role_is_superuser=row[1],
    )


async def verify_database_runtime(config: Settings = settings) -> None:
    """Read identity and max_connections from PostgreSQL; fail closed if unsafe."""
    if config.DATABASE_URL.startswith("sqlite"):
        return
    async with engine.connect() as connection:
        identity = (
            await connection.execute(
                text(
                    "SELECT current_user, rol.rolsuper "
                    "FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user"
                )
            )
        ).one()
        observed_max_connections = int(
            (await connection.execute(text("SHOW max_connections"))).scalar_one()
        )
    assert_database_runtime_is_safe(
        config,
        connected_user=identity[0],
        connected_role_is_superuser=identity[1],
        observed_max_connections=observed_max_connections,
    )


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
        except BaseException:
            await session.rollback()
            raise
        finally:
            await session.close()
