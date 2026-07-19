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
    connected_database: str,
    connected_user: str,
    role_can_login: bool,
    role_inherits_privileges: bool,
    connected_role_is_superuser: bool,
    role_can_create_database: bool,
    role_can_create_role: bool,
    role_can_replicate: bool,
    role_bypasses_rls: bool,
    role_has_memberships: bool,
    observed_max_connections: int,
) -> None:
    """Attest the connected role and server capacity before serving traffic."""
    expected_url = make_url(config.DATABASE_URL)
    expected_database = expected_url.database or ""
    expected_user = expected_url.username or ""
    if connected_database != expected_database:
        raise RuntimeError(
            "connected current_database does not match the effective DATABASE_URL database"
        )
    if connected_user != expected_user:
        raise RuntimeError(
            "connected current_user does not match the effective DATABASE_URL username"
        )
    _assert_role_is_least_privilege(
        role_name="application",
        role_can_login=role_can_login,
        role_inherits_privileges=role_inherits_privileges,
        role_is_superuser=connected_role_is_superuser,
        role_can_create_database=role_can_create_database,
        role_can_create_role=role_can_create_role,
        role_can_replicate=role_can_replicate,
        role_bypasses_rls=role_bypasses_rls,
        role_has_memberships=role_has_memberships,
    )
    required = configured_connection_total(config)
    if required > observed_max_connections:
        raise RuntimeError(
            f"configured aggregate {required} exceeds PostgreSQL max_connections "
            f"{observed_max_connections}"
        )


def assert_migrator_connection_is_safe(
    config: Settings,
    *,
    connected_database: str,
    connected_user: str,
    role_can_login: bool,
    role_inherits_privileges: bool,
    connected_role_is_superuser: bool,
    role_can_create_database: bool,
    role_can_create_role: bool,
    role_can_replicate: bool,
    role_bypasses_rls: bool,
    role_has_memberships: bool,
) -> None:
    expected_url = make_url(config.MIGRATOR_DATABASE_URL or config.DATABASE_URL)
    expected_database = expected_url.database or ""
    expected_user = expected_url.username or ""
    if connected_database != expected_database:
        raise RuntimeError(
            "connected migrator current_database does not match the effective migration URL database"
        )
    if connected_user != expected_user:
        raise RuntimeError(
            "connected migrator current_user does not match the effective migration URL username"
        )
    _assert_role_is_least_privilege(
        role_name="migrator",
        role_can_login=role_can_login,
        role_inherits_privileges=role_inherits_privileges,
        role_is_superuser=connected_role_is_superuser,
        role_can_create_database=role_can_create_database,
        role_can_create_role=role_can_create_role,
        role_can_replicate=role_can_replicate,
        role_bypasses_rls=role_bypasses_rls,
        role_has_memberships=role_has_memberships,
    )


def _assert_role_is_least_privilege(
    *,
    role_name: str,
    role_can_login: bool,
    role_inherits_privileges: bool,
    role_is_superuser: bool,
    role_can_create_database: bool,
    role_can_create_role: bool,
    role_can_replicate: bool,
    role_bypasses_rls: bool,
    role_has_memberships: bool,
) -> None:
    if role_is_superuser:
        raise RuntimeError(f"{role_name} database role must not be a superuser")
    if not role_can_login or any(
        (
            role_inherits_privileges,
            role_can_create_database,
            role_can_create_role,
            role_can_replicate,
            role_bypasses_rls,
        )
    ):
        raise RuntimeError(f"{role_name} database role must remain least-privilege")
    if role_has_memberships:
        raise RuntimeError(
            f"{role_name} database role membership permits SET ROLE escalation"
        )


_ROLE_ATTESTATION_SQL = text(
    "SELECT current_database(), current_user, rol.rolcanlogin, rol.rolinherit, "
    "rol.rolsuper, rol.rolcreatedb, rol.rolcreaterole, rol.rolreplication, "
    "rol.rolbypassrls, EXISTS ("
    "SELECT 1 FROM pg_catalog.pg_auth_members AS membership "
    "WHERE membership.member = rol.oid OR membership.roleid = rol.oid"
    ") FROM pg_catalog.pg_roles AS rol WHERE rol.rolname = current_user"
)


def verify_migrator_connection(connection, config: Settings = settings) -> None:
    expected_url = config.MIGRATOR_DATABASE_URL or config.DATABASE_URL
    if expected_url.startswith("sqlite"):
        return
    row = connection.execute(_ROLE_ATTESTATION_SQL).one()
    assert_migrator_connection_is_safe(
        config,
        connected_database=row[0],
        connected_user=row[1],
        role_can_login=row[2],
        role_inherits_privileges=row[3],
        connected_role_is_superuser=row[4],
        role_can_create_database=row[5],
        role_can_create_role=row[6],
        role_can_replicate=row[7],
        role_bypasses_rls=row[8],
        role_has_memberships=row[9],
    )


async def verify_database_runtime(config: Settings = settings) -> None:
    """Read identity and max_connections from PostgreSQL; fail closed if unsafe."""
    if config.DATABASE_URL.startswith("sqlite"):
        return
    async with engine.connect() as connection:
        identity = (await connection.execute(_ROLE_ATTESTATION_SQL)).one()
        observed_max_connections = int(
            (await connection.execute(text("SHOW max_connections"))).scalar_one()
        )
    assert_database_runtime_is_safe(
        config,
        connected_database=identity[0],
        connected_user=identity[1],
        role_can_login=identity[2],
        role_inherits_privileges=identity[3],
        connected_role_is_superuser=identity[4],
        role_can_create_database=identity[5],
        role_can_create_role=identity[6],
        role_can_replicate=identity[7],
        role_bypasses_rls=identity[8],
        role_has_memberships=identity[9],
        observed_max_connections=observed_max_connections,
    )


pool_kwargs = engine_options(settings)

engine = create_async_engine(
    settings.DATABASE_URL,
    echo=False,
    future=True,
    hide_parameters=True,
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
