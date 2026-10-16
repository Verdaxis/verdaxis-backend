"""No ambient URL, changed identity, runtime writer, or schema drift is accepted."""
import pytest
from sqlalchemy import text

from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['oid', 'column', 'role', 'same_database', 'baseline', 'cluster'])
async def test_copy_refuses_changed_authority(copy_case, change):
    from scripts.phase0.copy import CopyRefused, copy_database
    manifest, source, target, _ = copy_case
    if change == 'oid':
        manifest['execution']['target']['oid'] += 1
    elif change == 'column':
        await target.execute(text('ALTER TABLE notifications ADD COLUMN unreviewed text'))
        await target.commit()
    elif change == 'cluster':
        manifest['execution']['target']['cluster_system_identifier'] = '123'
    elif change == 'role':
        manifest['execution']['target']['role'] = 'verdaxis_app'
    elif change == 'same_database':
        manifest['execution']['target'] = manifest['execution']['source'].copy()
    else:
        manifest['execution']['baseline_reference_rows']['products'][0]['name'] = 'wrong'
    with pytest.raises(CopyRefused) as result:
        await copy_database(manifest, source, target)
    assert 'postgresql' not in str(result.value)
    assert (await target.execute(text('SELECT count(*) FROM users'))).scalar_one() == 0


@pytest.mark.asyncio
async def test_copy_as_read_only_role_refuses_before_writes(copy_case):
    from sqlalchemy.ext.asyncio import create_async_engine

    from scripts.phase0.copy import CopyRefused, copy_database
    manifest, source, target, _ = copy_case
    identity = manifest['execution']['source']
    role = identity['role']
    await target.execute(text(f'GRANT USAGE ON SCHEMA public TO "{role}"'))
    await target.execute(text(f'GRANT SELECT ON ALL TABLES IN SCHEMA public TO "{role}"'))
    await target.commit()
    engine = create_async_engine(target.engine.url.set(username=role))
    try:
        async with engine.connect() as runtime:
            with pytest.raises(CopyRefused):
                await copy_database(manifest, source, runtime)
        assert (await target.execute(text('SELECT count(*) FROM users'))).scalar_one() == 0
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_orphan_notice_evidence_blocks_copy(copy_case):
    from sqlalchemy.ext.asyncio import create_async_engine

    from scripts.phase0.copy import CopyRefused, copy_database
    manifest, source, target, _ = copy_case
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text("UPDATE audit_logs SET resource_id='00000000-0000-0000-0000-000000000000' WHERE action='kyc.grace_notice_sent'"))
        with pytest.raises(CopyRefused):
            await copy_database(manifest, source, target)
        assert (await target.execute(text('SELECT count(*) FROM users'))).scalar_one() == 0
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_legacy_source_value_refuses_before_any_target_write(copy_case):
    """A non-NULL legacy source-only column refuses with its named code.

    The target has no columns for `rfqs.reference_number`, `rfqs.daily_seq` or
    `rfq_quotes.last_counter_by`. A value in one of them would be dropped
    silently by a column-mapped copy, so the gate must refuse before the target
    transaction writes anything and must publish no receipt.
    """
    from pathlib import Path

    from sqlalchemy.ext.asyncio import create_async_engine

    from scripts.phase0.copy import copy_database
    from scripts.phase0.refusal import Refusal
    manifest, source, target, _ = copy_case
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text("UPDATE rfqs SET reference_number='RFQ-0001' WHERE id=(SELECT min(id) FROM rfqs)"))
        with pytest.raises(Refusal) as failure:
            await copy_database(manifest, source, target)
        assert failure.value.code == 'PHASE0_LEGACY_VALUE'
        receipt = Path(manifest['execution']['receipt_path'])
        assert not receipt.exists()
        assert not receipt.with_suffix('.attempt.json').exists()
        assert not receipt.with_suffix('.failure.json').exists()
        # The refusal happens inside the source transaction, before the target
        # writes: the target keeps its pristine bootstrap state.
        assert (await target.execute(text('SELECT count(*) FROM users'))).scalar_one() == 0
        assert (await target.execute(text('SELECT count(*) FROM rfqs'))).scalar_one() == 0
        await target.rollback()
        # The source value itself is preserved: the tool never corrects data.
        await source.rollback()
        async with engine.connect() as owner:
            kept = (await owner.execute(text("SELECT count(*) FROM rfqs WHERE reference_number IS NOT NULL"))).scalar_one()
        assert kept == 1
        await owner.rollback()
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_null_legacy_columns_copy_and_record_the_null_proof(copy_case):
    from pathlib import Path

    from scripts.phase0.copy import copy_database
    manifest, source, target, _ = copy_case
    # The fixture builds every legacy column NULL, exactly like live today.
    for table, column in (('rfqs', 'reference_number'), ('rfqs', 'daily_seq'), ('rfq_quotes', 'last_counter_by')):
        owner_check = (await source.execute(text(
            f'SELECT count(*) FROM {table} WHERE {column} IS NOT NULL'))).scalar_one()
        assert owner_check == 0
    await source.rollback()
    receipt = await copy_database(manifest, source, target)
    assert receipt.legacy_columns == {'rfqs.reference_number': 0, 'rfqs.daily_seq': 0,
                                      'rfq_quotes.last_counter_by': 0}
    published = Path(manifest['execution']['receipt_path'])
    assert published.is_file()
    import json
    recorded = json.loads(published.read_text())['legacy_columns']
    assert recorded == receipt.legacy_columns
    await target.rollback()


def test_secret_file_and_url_boundaries(tmp_path):
    from scripts.phase0.copy import CopyRefused, read_private_file, validate_url
    path = tmp_path / 'secret'
    path.write_text('DATABASE_URL=secret')
    path.chmod(0o644)
    with pytest.raises(CopyRefused):
        read_private_file(path)
    path.chmod(0o600)
    link = tmp_path / 'link'
    link.symlink_to(path)
    with pytest.raises(CopyRefused):
        read_private_file(link)
    assert read_private_file(path) == 'DATABASE_URL=secret'
    for url in ('postgresql://a:b@169.58.37.164/verdaxis', 'postgresql://a:b@127.0.0.1:5555/test?host=evil'):
        with pytest.raises(CopyRefused):
            validate_url(url, dict(mode='synthetic'))
