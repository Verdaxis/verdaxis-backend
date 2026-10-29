"""Build synthetic history with the pinned migration tree, never ORM DDL."""
import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path

from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from scripts.phase0.manifest import PROD_SHA, STAGING_SHA, digest, inspect_catalog

ROOT = Path(__file__).resolve().parents[2]


async def historical(connection):
    from tests.phase0.synthetic import TIME, uid

    await connection.execute(text("""
        INSERT INTO organizations (id, name, type, created_at) VALUES
        ('10000000-0000-4000-8000-000000000001', 'Synthetic buyer', 'FUEL_BUYER', '2026-01-01'),
        ('10000000-0000-4000-8000-000000000002', 'Synthetic seller', 'FUEL_SUPPLIER', '2026-01-01');
    """))
    for status in ('PENDING_CONFIRMATION', 'CONFIRMED', 'DELIVERED', 'PAID', 'CANCELLED', 'DECLINED'):
        await connection.execute(text("""
            INSERT INTO trades (id, created_at, buyer_id, seller_id, initiated_by, quantity_mt, price_per_mt_usd,
                status, confirmed_at, delivered_at, paid_at, final_quantity_mt, final_price_per_mt, final_total_usd)
            VALUES (:id, :created_at, '10000000-0000-4000-8000-000000000001', '10000000-0000-4000-8000-000000000002',
                'BUYER', 10, 700, CAST(:status AS varchar),
                CASE WHEN CAST(:status AS varchar) IN ('CONFIRMED','DELIVERED','PAID') THEN '2026-01-01'::timestamptz END,
                CASE WHEN CAST(:status AS varchar) IN ('DELIVERED','PAID') THEN '2026-01-02'::timestamptz END,
                CASE WHEN CAST(:status AS varchar) = 'PAID' THEN '2026-01-03'::timestamptz END,
                CASE WHEN CAST(:status AS varchar) IN ('DELIVERED','PAID') THEN 10 END,
                CASE WHEN CAST(:status AS varchar) IN ('DELIVERED','PAID') THEN 700 END,
                CASE WHEN CAST(:status AS varchar) IN ('DELIVERED','PAID') THEN 7000 END)
        """), {"status": status, "id": uid('historical-' + status), "created_at": TIME})
    # Live production keeps five legacy RFQ objects the target schema has no
    # columns for. Recreate exactly those objects with live's column positions
    # and NULL values, so the synthetic source matches the declared catalog.
    # asyncpg runs one statement per execute, so each DDL is separate.
    for statement in (
        "ALTER TABLE rfqs ADD COLUMN reference_number varchar(20)",
        "ALTER TABLE rfqs ADD COLUMN daily_seq integer",
        "ALTER TABLE rfqs ADD CONSTRAINT rfqs_reference_number_key UNIQUE (reference_number)",
        "CREATE INDEX ix_rfqs_reference_number ON rfqs (reference_number)",
        "ALTER TABLE rfq_quotes ADD COLUMN last_counter_by varchar(10) DEFAULT NULL",
    ):
        await connection.execute(text(statement))


async def inspect(name):
    url = f"postgresql+asyncpg://phase0_owner:{os.environ['PHASE0_PASSWORD']}@127.0.0.1:{os.environ['PHASE0_PORT']}/phase0_{name}_test"
    engine = create_async_engine(url)
    try:
        if name == "prod":
            async with engine.begin() as connection:
                await historical(connection)
            tree = Path(os.environ["PHASE0_WORK"]) / name
            subprocess.run([sys.executable, "-m", "alembic", "upgrade", "kycg_20261014_prod"],
                           cwd=tree, env=dict(os.environ, DATABASE_URL=url, MIGRATOR_DATABASE_URL=url), check=True)
        async with engine.begin() as connection:
            await connection.execute(text("SET LOCAL TIME ZONE 'UTC'"))
            catalog = await connection.run_sync(inspect_catalog)
            (Path(os.environ["PHASE0_WORK"]) / f"{name}-catalog.json").write_text(json.dumps(catalog, indent=2))
            if name == "prod":
                from tests.phase0.synthetic import populate
                await populate(connection, catalog)
                catalog = await connection.run_sync(inspect_catalog)
                (Path(os.environ["PHASE0_WORK"]) / "prod-populated-catalog.json").write_text(json.dumps(catalog, indent=2))
            schema = {key: value for key, value in catalog.items() if key != 'reference_rows'}
            receipt = {'source_sha': PROD_SHA if name == 'prod' else STAGING_SHA,
                       'revision': catalog['revision'], 'schema_sha256': digest(schema),
                       'reference_rows_sha256': digest(catalog['reference_rows']),
                       'table_count': len(catalog['tables'])}
            (Path(os.environ['PHASE0_WORK']) / f'{name}-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
    finally:
        await engine.dispose()


async def build_union():
    import asyncpg

    admin = await asyncpg.connect(host='127.0.0.1', port=int(os.environ['PHASE0_PORT']),
                                 user='postgres', password=os.environ['PHASE0_PASSWORD'], database='postgres')
    try:
        await admin.execute('CREATE DATABASE phase0_union_test OWNER phase0_owner')
    finally:
        await admin.close()
    url = f"postgresql+asyncpg://phase0_owner:{os.environ['PHASE0_PASSWORD']}@127.0.0.1:{os.environ['PHASE0_PORT']}/phase0_union_test"
    # Compare the historical union against its own migration and model tree.
    # Current-head drift is checked separately by tests/postgres/test_alembic_check.py.
    frozen_sha = '34dacf327d56ec32ddaf74c80390986a4fe1061d'
    tree = Path(os.environ['PHASE0_WORK']) / 'union-source'
    tree.mkdir()
    archive = tree.parent / 'union-source.tar'
    with archive.open('wb') as output:
        subprocess.run(['git', 'archive', frozen_sha], cwd=ROOT, stdout=output, check=True)
    subprocess.run(['tar', '-xf', str(archive), '-C', str(tree)], check=True)
    (tree / 'venv').symlink_to(ROOT / 'venv', target_is_directory=True)
    for arguments in (('upgrade', 'p0_20261016_union'), ('check',)):
        subprocess.run([sys.executable, '-m', 'alembic', *arguments],
                       cwd=tree, env=dict(os.environ, DATABASE_URL=url, MIGRATOR_DATABASE_URL=url), check=True)
    engine = create_async_engine(url)
    try:
        async with engine.connect() as connection:
            catalog = await connection.run_sync(inspect_catalog)
            (Path(os.environ['PHASE0_WORK']) / 'union-catalog.json').write_text(json.dumps(catalog, indent=2) + '\n')
    finally:
        await engine.dispose()


if __name__ == "__main__":
    for name in ("prod", "staging"):
        asyncio.run(inspect(name))
    asyncio.run(build_union())
