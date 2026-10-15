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
