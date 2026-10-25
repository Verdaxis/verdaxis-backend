"""Run continuing application protocols with a separate Rust lock contender."""
import os
import subprocess
import sys
from pathlib import Path


def main():
    from sqlalchemy.engine import make_url
    url = make_url(os.environ["RUST_TEST_MIGRATOR_DATABASE_URL"])
    assert url.host == "127.0.0.1" and url.database.endswith("_test")
    admin = url.set(drivername="postgresql+asyncpg", database="postgres")
    target = url.set(drivername="postgresql+asyncpg", database="p89_protocol_market_integrity_test",username="rust_migrator")
    import asyncio

    from sqlalchemy import text
    from sqlalchemy.ext.asyncio import create_async_engine

    async def prepare():
        engine = create_async_engine(admin, isolation_level="AUTOCOMMIT")
        async with engine.connect() as connection:
            await connection.execute(text("ALTER ROLE rust_migrator NOINHERIT"))
            await connection.execute(text("GRANT SET ON PARAMETER session_replication_role TO rust_migrator"))
            exists = await connection.scalar(text("SELECT 1 FROM pg_database WHERE datname='p89_protocol_market_integrity_test'"))
            if not exists:
                await connection.execute(text("CREATE DATABASE p89_protocol_market_integrity_test OWNER rust_migrator"))
        await engine.dispose()
    asyncio.run(prepare())
    env = dict(os.environ, ENVIRONMENT="test", RELEASE_SHA=os.environ["RELEASE_SHA"],
               JWT_SECRET="disposable-protocol-signing-material-at-least-32-bytes",
               MARKET_INTEGRITY_TEST_DATABASE_URL=target.render_as_string(hide_password=False),
               PRODUCT_ANALYTICS_TEST_DATABASE_URL=target.set(database="p89_protocol_analytics_test").render_as_string(hide_password=False),
               DATABASE_URL=target.render_as_string(hide_password=False),
               MIGRATOR_DATABASE_URL=target.render_as_string(hide_password=False),
               POSTGRES_ADMIN_TEST_DATABASE_URL=admin.render_as_string(hide_password=False),
               PYTHONPATH=str(Path(__file__).resolve().parent)+os.pathsep+str(Path.cwd()))
    asyncio.run(_analytics(admin))
    async def extensions():
        for name in ["p89_protocol_market_integrity_test", "p89_protocol_analytics_test", "template1"]:
            engine=create_async_engine(admin.set(database=name))
            async with engine.begin() as connection:
                await connection.execute(text("CREATE EXTENSION IF NOT EXISTS postgis"))
                await connection.execute(text("CREATE EXTENSION IF NOT EXISTS pgcrypto"))
            await engine.dispose()
    asyncio.run(extensions())
    for database in [target, target.set(database="p89_protocol_analytics_test")]:
        migration_env={**env,"DATABASE_URL":database.render_as_string(hide_password=False),"MIGRATOR_DATABASE_URL":database.render_as_string(hide_password=False)}
        subprocess.run([sys.executable,"-m","alembic","upgrade","head"],env=migration_env,check=True)
    async def grants():
        engine=create_async_engine(admin.set(database=target.database))
        async with engine.begin() as connection:
            await connection.execute(text("GRANT USAGE ON SCHEMA public TO rust_app"))
            await connection.execute(text("GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA public TO rust_app"))
            await connection.execute(text("GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA public TO rust_app"))
        await engine.dispose()
    asyncio.run(grants())
    env["DATABASE_URL"]=target.set(username="rust_app").render_as_string(hide_password=False)
    env["RUNTIME_TEST_APP_ROLE"]="rust_app"
    targets = [
        "tests/postgres/test_order_expiry_reminders_concurrency.py",
        "tests/postgres/test_demo_market_seed.py",
        "tests/postgres/test_market_routes.py::test_bilateral_execution_is_permanently_disabled_but_one_sided_read_is_safe",
        "tests/postgres/test_supplier_offers.py::test_supplier_offer_revision_and_withdrawal_reject_stale_forms",
        "tests/postgres/test_targeted_rfq.py::test_targeted_rfq_captures_server_source_and_preserves_snapshot",
        "tests/postgres/test_market_migrations.py::test_parent_shape_sentinel_requires_explicit_cli_quarantine_then_upgrades",
    ]
    result = subprocess.run([sys.executable,"-m","pytest","-q","-s","-p","phase8_protocol_plugin",*targets], env=env)
    raise SystemExit(result.returncode)


async def _analytics(admin):
    from sqlalchemy import text
    from sqlalchemy.ext.asyncio import create_async_engine
    engine=create_async_engine(admin,isolation_level="AUTOCOMMIT")
    async with engine.connect() as connection:
        if not await connection.scalar(text("SELECT 1 FROM pg_database WHERE datname='p89_protocol_analytics_test'")):
            await connection.execute(text("CREATE DATABASE p89_protocol_analytics_test OWNER rust_migrator"))
    await engine.dispose()


if __name__ == "__main__":
    main()
