"""Final union schema, built independently and upgraded without rewriting history."""
import os
import subprocess
import sys
from pathlib import Path

import pytest
from alembic.config import Config
from alembic.script import ScriptDirectory
from sqlalchemy import text

from scripts.phase0.manifest import inspect_catalog
from tests.phase0.test_bootstrap import target, url  # noqa: F401

ROOT = Path(__file__).resolve().parents[2]
REVISION = 'p0_20261016_union'


def migrate(database, revision=REVISION):
    environment = dict(os.environ, DATABASE_URL=url(database), MIGRATOR_DATABASE_URL=url(database))
    subprocess.run([sys.executable, '-m', 'alembic', 'upgrade', revision],
                   cwd=ROOT, env=environment, check=True, capture_output=True)


def test_union_checkpoint_contract():
    graph = ScriptDirectory.from_config(Config(str(ROOT / 'alembic.ini')))
    assert graph.get_current_head() == REVISION
    assert graph.get_revision(REVISION).down_revision == 'kycd_20261015_staging'
    rows = (ROOT / 'deploy/migration-checkpoints.tsv').read_text().splitlines()
    assert f'kycd_20261015_staging\t{REVISION}' in rows
    assert f'{REVISION}\t{REVISION}' in rows
    assert not any('base' in row or 'EMPTY' in row for row in rows if not row.startswith('#'))


@pytest.mark.asyncio
async def test_union_fresh_and_populated_upgrade(target):
    engine, identity = target
    migrate(identity['database'], 'kycd_20261015_staging')
    async with engine.begin() as connection:
        await connection.execute(text("""
            INSERT INTO organizations (id, name, type)
            VALUES ('10000000-0000-4000-8000-000000000009', 'Union history', 'FUEL_BUYER')
        """))
        before = (await connection.execute(text('SELECT row_to_json(o)::text FROM organizations o'))).scalar_one()
    migrate(identity['database'])
    async with engine.connect() as connection:
        catalog = await connection.run_sync(inspect_catalog)
        assert catalog['revision'] == REVISION
        assert (await connection.execute(text('SELECT row_to_json(o)::text FROM organizations o'))).scalar_one() == before
        assert (await connection.execute(text('SELECT count(*) FROM kyc_execution_grace'))).scalar_one() == 0
        assert not {'user_browsing_events', 'user_activity_delivery_reports'} & set(catalog['tables'])
        products = dict((await connection.execute(text('SELECT name, id::text FROM products'))).all())
        assert products['B30'] == 'c4cecebc-3d2d-5840-8021-57a9a11bc673'
        assert products['B100'] == '74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5'
        assert products['UCOME B100'] == 'e561e43f-d9b2-598e-981c-f1d28d515ddc'
