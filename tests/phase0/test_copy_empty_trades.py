"""Guard proof must not depend on customer trade history."""
import pytest
from sqlalchemy import text
from sqlalchemy.ext.asyncio import create_async_engine

from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
async def test_copy_without_trades_preserves_source_and_leaves_no_probe_rows(copy_case):
    from scripts.phase0.copy import copy_database
    from scripts.phase0.verify import fingerprint_database
    manifest, source, target, _ = copy_case
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            await owner.execute(text("UPDATE rfqs SET status='OPEN', accepted_quote_id=NULL, trade_id=NULL"))
            await owner.execute(text('UPDATE commissions SET trade_id=NULL'))
            await owner.execute(text('DELETE FROM trades'))
    finally:
        await engine.dispose()
    before = await fingerprint_database(source, manifest['source_catalog'])
    await source.rollback()
    receipt = await copy_database(manifest, source, target)
    assert receipt.verification.tables['trades']['count'] == 0
    assert await fingerprint_database(source, manifest['source_catalog']) == before
    assert (await target.execute(text('SELECT count(*) FROM trades'))).scalar_one() == 0
    assert (await target.execute(text('SELECT count(*) FROM organizations'))).scalar_one() == before['tables']['organizations']['count']


@pytest.mark.asyncio
async def test_guard_probe_refuses_disabled_trigger_and_cleans_up(copy_case):
    from scripts.phase0.copy import CopyRefused, probe_trade_guard
    _, _, target, _ = copy_case
    await target.execute(text('ALTER TABLE trades DISABLE TRIGGER trades_snapshot_guard'))
    with pytest.raises(CopyRefused, match='did not reject'):
        await probe_trade_guard(target)
    assert (await target.execute(text('SELECT count(*) FROM organizations'))).scalar_one() == 0
    assert (await target.execute(text('SELECT count(*) FROM trades'))).scalar_one() == 0


@pytest.mark.asyncio
async def test_guard_probe_on_empty_database_rolls_back_its_parties(copy_case):
    from scripts.phase0.copy import probe_trade_guard
    _, _, target, _ = copy_case
    await probe_trade_guard(target)
    assert (await target.execute(text('SELECT count(*) FROM organizations'))).scalar_one() == 0
    assert (await target.execute(text('SELECT count(*) FROM trades'))).scalar_one() == 0
