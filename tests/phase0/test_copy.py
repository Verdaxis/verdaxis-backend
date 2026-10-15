"""Task 5 copy proofs against independently migrated synthetic databases."""
import copy
import json
import os
from pathlib import Path
from uuid import uuid4

import asyncpg
import pytest
import pytest_asyncio
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from scripts.phase0.manifest import inspect_catalog, load_manifest


@pytest_asyncio.fixture
async def copy_case(tmp_path):
    port = int(os.environ['PHASE0_PORT'])
    password = os.environ['PHASE0_PASSWORD']
    admin = await asyncpg.connect(host='127.0.0.1', port=port, user='postgres', password=password, database='postgres')
    suffix = uuid4().hex[:12]
    source_name, target_name = f'phase0_source_{suffix}', f'phase0_target_{suffix}'
    backup = f'phase0_backup_{suffix}'
    await admin.execute(f'CREATE ROLE {backup} LOGIN NOINHERIT PASSWORD \'{password}\'')
    for name, template in ((source_name, 'phase0_prod_test'), (target_name, 'phase0_staging_test')):
        await admin.execute(f'CREATE DATABASE {name} OWNER phase0_owner TEMPLATE {template}')
    engines = []
    async def connect(name, role='phase0_owner'):
        engine = create_async_engine(f'postgresql+asyncpg://{role}:{password}@127.0.0.1:{port}/{name}')
        engines.append(engine)
        return await engine.connect()
    owner = await connect(source_name)
    await owner.execute(text(f'GRANT USAGE ON SCHEMA public TO {backup}'))
    await owner.execute(text(f'GRANT SELECT ON ALL TABLES IN SCHEMA public TO {backup}'))
    await owner.execute(text(f'GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO {backup}'))
    await owner.commit()
    await owner.close()
    source, target = await connect(source_name, backup), await connect(target_name)
    manifest = copy.deepcopy(load_manifest())
    baseline = await target.run_sync(inspect_catalog)
    await target.rollback()
    system_id = str(await admin.fetchval('SELECT system_identifier FROM pg_control_system()'))
    async def identity(name, role):
        return dict(host='127.0.0.1', port=port, database=name,
                    oid=await admin.fetchval('SELECT oid FROM pg_database WHERE datname=$1', name),
                    role=role, cluster_system_identifier=system_id)
    manifest['execution'] = dict(mode='synthetic', host_id=os.uname().nodename,
        source=await identity(source_name, backup), target=await identity(target_name, 'phase0_owner'),
        baseline_reference_rows=baseline['reference_rows'],
        baseline_sequences={s['name']: {'last_value': int(s['start']), 'is_called': False} for s in baseline['sequences']},
        receipt_path=str(tmp_path / 'receipt.json'))
    try:
        yield manifest, source, target, admin
    finally:
        await source.close()
        await target.close()
        for engine in engines:
            await engine.dispose()
        for name in (source_name, target_name):
            await admin.execute(f'DROP DATABASE {name} WITH (FORCE)')
        await admin.execute(f'DROP ROLE {backup}')
        await admin.close()


@pytest.mark.asyncio
async def test_copy_cycles_and_next_sequence_values(copy_case):
    from scripts.phase0.copy import CopyRefused, copy_database
    manifest, source, target, _ = copy_case
    receipt = await copy_database(manifest, source, target)
    assert len(receipt.verification.tables) == 57
    assert all(v['count'] > 0 for v in receipt.verification.tables.values())
    assert (await target.execute(text("SELECT nextval('market_event_stream_seq')"))).scalar_one() == 151
    assert (await target.execute(text("SELECT nextval('orderbook_acceptance_ordinal_seq')"))).scalar_one() == 201
    assert (await target.execute(text('SELECT count(*) FROM rfqs r JOIN rfq_quotes q ON r.accepted_quote_id=q.id AND q.rfq_id=r.id'))).scalar_one() == 1
    assert (await target.execute(text('SELECT count(*) FROM commissions c JOIN orders o ON c.match_id=o.id'))).scalar_one() == 1
    assert (await target.execute(text('SELECT count(*) FROM orderbook_orders o JOIN market_support_authorizations a ON o.support_authorization_id=a.id WHERE a.status=\'CONSUMED\''))).scalar_one() == 1
    assert (await target.execute(text('SELECT count(*) FROM kyc_grace_notices'))).scalar_one() == 1
    for table in ('refresh_sessions', 'supplier_offers', 'kyc_documents'):
        assert (await target.execute(text(f'SELECT count(*) FROM {table}'))).scalar_one() == 0
    await target.rollback()
    with pytest.raises(CopyRefused):
        await copy_database(manifest, source, target)
    assert Path(manifest['execution']['receipt_path']).stat().st_mode & 0o777 == 0o600


@pytest.mark.asyncio
@pytest.mark.parametrize('boundary', ['before_data', 'during_cycle', 'after_trigger_restoration', 'after_sequences', 'after_commit', 'before_receipt'])
async def test_copy_failure_boundaries(copy_case, boundary):
    from scripts.phase0.copy import CopyRefused, copy_database
    from scripts.phase0.verify import fingerprint_database
    manifest, source, target, _ = copy_case
    before = await fingerprint_database(source, manifest['source_catalog'])
    await source.rollback()
    with pytest.raises(CopyRefused):
        await copy_database(manifest, source, target, fail_at=boundary)
    failure = json.loads(Path(manifest['execution']['receipt_path']).with_suffix('.failure.json').read_text())
    assert failure['stage'] == boundary
    assert failure['error'] == f'Injected failure: {boundary}'
    restored = await target.run_sync(inspect_catalog)
    from scripts.phase0.manifest import require_complete_inventory
    require_complete_inventory(restored, manifest['target_catalog'])
    await target.rollback()
    if boundary in ('after_sequences', 'after_commit', 'before_receipt'):
        # A different receipt path cannot turn sequence-mutated or populated
        # storage into a pristine target after a crash.
        retry_manifest = copy.deepcopy(manifest)
        retry_manifest['execution']['receipt_path'] += '.retry'
        with pytest.raises(CopyRefused):
            await copy_database(retry_manifest, source, target)
    after = await fingerprint_database(source, manifest['source_catalog'])
    assert before == after
    await source.rollback()
    with pytest.raises(CopyRefused):
        await copy_database(manifest, source, target)


@pytest.mark.asyncio
async def test_exact_text_codecs_and_catalog_values(copy_case):
    from scripts.phase0.copy import copy_database
    manifest, source, target, _ = copy_case
    await copy_database(manifest, source, target)
    rows = (await target.execute(text("SELECT data::text FROM notifications WHERE title='Synthetic codec' ORDER BY data::text NULLS FIRST"))).scalars().all()
    assert rows == [None, 'null', '{"value":1234567890.12345678901234567890}']
    assert (await target.execute(text("SELECT timezone FROM delivery_points WHERE name='Singapore'"))).scalar_one() == 'Asia/Singapore'
    assert (await target.execute(text("SELECT spec_description FROM products WHERE name='Synthetic retired product'"))).scalar_one() == 'Preserve this source description'


@pytest.mark.asyncio
@pytest.mark.parametrize('last,called,maximum,cache,expected', [
    (1, False, None, 1, 1), (1, False, 1, 1, 2),
    (32, True, 7, 32, 33), (32, True, 91, 32, 92),
])
async def test_postgres_sequence_next_value(copy_case, last, called, maximum, cache, expected):
    from scripts.phase0.codecs import next_sequence_state
    _, _, target, _ = copy_case
    await target.execute(text(f'CREATE SEQUENCE copy_sequence_probe CACHE {cache}'))
    definition = dict(increment='1', minimum='1', maximum='9223372036854775807', cache=str(cache), cycle=False)
    value, new_called, next_value = next_sequence_state(definition, last, called, maximum)
    await target.execute(text("SELECT setval('copy_sequence_probe', :value, :called)"), {'value': value, 'called': new_called})
    assert next_value == expected
    assert (await target.execute(text("SELECT nextval('copy_sequence_probe')"))).scalar_one() == expected


def test_sequence_high_water_cases():
    from scripts.phase0.codecs import next_sequence_state
    definition = dict(increment='1', minimum='1', maximum='1000000', cache='32', cycle=False)
    assert next_sequence_state(definition, 1, False, None) == (1, False, 1)
    assert next_sequence_state(definition, 1, False, 1) == (1, True, 2)
    assert next_sequence_state(definition, 32, True, 7) == (32, True, 33)
    assert next_sequence_state(definition, 32, True, 91) == (91, True, 92)
