"""Disposable PostgreSQL 17 schema and drift mutation checks."""

import pytest
from sqlalchemy import (
    Column,
    Enum,
    ForeignKey,
    Index,
    Integer,
    MetaData,
    String,
    Table,
    text,
)
from sqlalchemy.ext.asyncio import create_async_engine
from alembic.autogenerate import compare_metadata
from alembic.migration import MigrationContext

from app.migration_drift import compare_server_default, compare_type, include_object


@pytest.mark.asyncio
async def test_postgresql17_runtime_drift_mutations_are_not_suppressed(analytics_pg_url):
    engine = create_async_engine(analytics_pg_url)
    try:
        async with engine.begin() as connection:
            version, postgis = (
                await connection.execute(
                    text("SELECT version(), PostGIS_Full_Version()")
                )
            ).one()
            assert "PostgreSQL 17." in version
            assert 'POSTGIS="3.6.' in postgis

            await connection.execute(text("DROP SCHEMA IF EXISTS runtime_drift_probe CASCADE"))
            await connection.execute(text("CREATE SCHEMA runtime_drift_probe"))
            await connection.execute(
                text(
                    "CREATE TABLE runtime_drift_probe.parents "
                    "(id integer PRIMARY KEY)"
                )
            )
            await connection.execute(
                text(
                    "CREATE TABLE runtime_drift_probe.orders "
                    "(id integer PRIMARY KEY, "
                    "status varchar(16) NOT NULL CHECK (status IN ('OPEN', 'BROKEN')), "
                    "amount integer NOT NULL, "
                    "parent_id integer, "
                    "wrong_default boolean DEFAULT true)"
                )
            )

            def compare(sync_connection):
                metadata = MetaData(schema="runtime_drift_probe")
                Table(
                    "parents",
                    metadata,
                    Column("id", Integer, primary_key=True),
                )
                orders = Table(
                    "orders",
                    metadata,
                    Column("id", Integer, primary_key=True),
                    Column(
                        "status",
                        Enum("OPEN", "CLOSED", native_enum=False, length=16),
                        nullable=False,
                    ),
                    Column("amount", String(20), nullable=False),
                    Column("parent_id", Integer, ForeignKey("runtime_drift_probe.parents.id")),
                    Column("wrong_default", String(5), server_default=text("false")),
                    schema="runtime_drift_probe",
                )
                Index("ix_runtime_drift_probe_orders_amount", orders.c.amount)
                reflected_orders = Table(
                    "orders",
                    MetaData(),
                    schema="runtime_drift_probe",
                    autoload_with=sync_connection,
                )
                assert compare_type(
                    None,
                    reflected_orders.c.status,
                    orders.c.status,
                    reflected_orders.c.status.type,
                    orders.c.status.type,
                ) is True  # same width, different PostgreSQL CHECK value set

                def include_probe(object_, name, type_, reflected, compare_to):
                    table = getattr(object_, "table", None)
                    if type_ == "schema":
                        return name == "runtime_drift_probe"
                    if type_ == "table":
                        return getattr(object_, "schema", None) == "runtime_drift_probe"
                    return getattr(table, "schema", None) == "runtime_drift_probe"

                context = MigrationContext.configure(
                    sync_connection,
                    opts={
                        "target_metadata": metadata,
                        "include_schemas": True,
                        "include_object": include_probe,
                        "compare_type": compare_type,
                        "compare_server_default": compare_server_default,
                    },
                )
                return compare_metadata(context, metadata)

            differences = await connection.run_sync(compare)
            def operation_names(value):
                if isinstance(value, (tuple, list)):
                    if value and isinstance(value[0], str):
                        yield value[0]
                    else:
                        for child in value:
                            yield from operation_names(child)

            operations = list(operation_names(differences))
            assert "modify_type" in operations  # enum/value and ordinary type drift
            assert "modify_default" in operations
            assert "add_fk" in operations
            assert "add_index" in operations
            await connection.execute(text("DROP SCHEMA runtime_drift_probe CASCADE"))
    finally:
        await engine.dispose()
