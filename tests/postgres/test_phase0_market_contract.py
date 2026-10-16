"""Evaluate the union qualification SQL on runner-owned PostgreSQL, not SQLite."""
import os
from uuid import UUID

import pytest
from sqlalchemy import Boolean, Column, MetaData, Numeric, String, Table, select
from sqlalchemy.dialects.postgresql import UUID as PG_UUID
from sqlalchemy.ext.asyncio import create_async_engine

from app.services import execution_policy
from tests.unit.test_phase0_catalog import BIOFUELS, biofuel_order


@pytest.mark.asyncio
@pytest.mark.parametrize("code", BIOFUELS)
@pytest.mark.parametrize("changes", [
    {}, {"specification_standard": "IMPCA"}, {"carbon_intensity_method": " \t\n"},
    {"carbon_intensity_gco2_mj": None}, {"off_spec": True},
    {"msds_available": False}, {"certification_declared": False},
    {"certification_scheme": " "}, {"origin": " "}, {"feedstock": None},
])
async def test_biofuel_sql_python_qualification_parity(code, changes):
    port = os.environ.get("PHASE0_PORT")
    password = os.environ.get("PHASE0_PASSWORD")
    assert port and password, "Use scripts/run_phase0_postgres_tests.sh"
    assert code in execution_policy.PRODUCTS_BY_ID[UUID(BIOFUELS[code][0])].name
    engine = create_async_engine(
        f"postgresql+asyncpg://phase0_owner:{password}@127.0.0.1:{port}/phase0_staging_test"
    )
    metadata = MetaData()
    record = Table(
        "phase0_qualification", metadata,
        Column("product_id", PG_UUID), Column("specification_standard", String),
        Column("carbon_intensity_method", String), Column("carbon_intensity_gco2_mj", Numeric),
        Column("off_spec", Boolean), Column("msds_available", Boolean),
        Column("certification_declared", Boolean), Column("certification_scheme", String),
        Column("origin", String), Column("feedstock", String), prefixes=["TEMPORARY"],
    )
    order = biofuel_order(code, **changes)
    try:
        async with engine.connect() as connection:
            async with connection.begin():
                await connection.run_sync(metadata.create_all)
                await connection.execute(record.insert().values(**{
                    column.name: getattr(order, column.name) for column in record.columns
                }))
                actual = await connection.scalar(select(
                    execution_policy.biofuel_supplier_metadata_clause(record.c, record.c.product_id)
                ))
                assert bool(actual) == execution_policy.order_is_execution_qualified(order)
                await connection.run_sync(metadata.drop_all)
    finally:
        await engine.dispose()
