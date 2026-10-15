"""Never accept counts or hashes from a row-filtered source."""
from pathlib import Path

import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
@pytest.mark.parametrize('side,mode', [('source', 'ENABLE'), ('source', 'FORCE'), ('target', 'FORCE')])
async def test_copy_refuses_unreviewed_row_security(copy_case, side, mode):
    from scripts.phase0.copy import CopyRefused, copy_database
    manifest, source, target, _ = copy_case
    connection = source if side == 'source' else target
    engine = create_async_engine(connection.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text(f'ALTER TABLE notifications {mode} ROW LEVEL SECURITY'))
            if mode == 'ENABLE':
                await owner.execute(text('CREATE POLICY hide_notifications ON notifications FOR SELECT USING (false)'))
    finally:
        await engine.dispose()
    with pytest.raises(CopyRefused):
        await copy_database(manifest, source, target)
    assert not Path(manifest['execution']['receipt_path']).with_suffix('.attempt.json').exists()
    assert (await target.execute(text('SELECT count(*) FROM users'))).scalar_one() == 0


@pytest.mark.asyncio
async def test_standalone_verification_refuses_filtered_source(copy_case):
    from scripts.phase0.copy import copy_database
    from scripts.phase0.verify import verify_database
    manifest, source, target, _ = copy_case
    await copy_database(manifest, source, target)
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text('ALTER TABLE notifications ENABLE ROW LEVEL SECURITY'))
            await owner.execute(text('CREATE POLICY hide_notifications ON notifications FOR SELECT USING (false)'))
    finally:
        await engine.dispose()
    await target.execute(text('DELETE FROM notifications'))
    await target.commit()
    with pytest.raises(ValueError, match='Row-level security'):
        await verify_database(manifest, source, target)
