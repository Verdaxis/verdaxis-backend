"""Historical guards must be restored without rewriting frozen evidence."""
import pytest
from sqlalchemy import text

from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
async def test_historical_snapshot_import_restores_exact_guards(copy_case):
    from scripts.phase0.copy import copy_database
    from scripts.phase0.manifest import inspect_catalog, require_complete_inventory
    manifest, source, target, _ = copy_case
    await copy_database(manifest, source, target)
    require_complete_inventory(await target.run_sync(inspect_catalog), manifest['target_catalog'])
    assert (await target.execute(text('SELECT count(*) FROM trades WHERE market_snapshot_version IS NULL'))).scalar_one() == 6
    assert (await target.execute(text("SELECT count(*) FROM trades t JOIN organizations o ON o.id=t.buyer_id WHERE t.market_snapshot_version=1 AND t.buyer_provenance='REAL' AND o.verification_status='REJECTED'"))).scalar_one() == 1
    assert (await target.execute(text("SELECT count(*) FROM kyc_execution_grace WHERE expires_at='2026-01-15'::timestamptz"))).scalar_one() == 1
    assert (await target.execute(text("SELECT response_body->>'_private' FROM market_command_results"))).scalar_one() == 'preserve'
    async with target.begin_nested() as savepoint:
        with pytest.raises(Exception):
            await target.execute(text("INSERT INTO trades (id,buyer_id,seller_id,initiated_by,quantity_mt,price_per_mt_usd,status) SELECT gen_random_uuid(),buyer_id,seller_id,initiated_by,quantity_mt,price_per_mt_usd,status FROM trades LIMIT 1"))
        await savepoint.rollback()
