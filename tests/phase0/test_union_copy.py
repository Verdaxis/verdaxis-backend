"""Final-union histories and cyclic references across COPY batch boundaries."""
import pytest
from sqlalchemy import text

from scripts.phase0.copy import copy_database


@pytest.mark.asyncio
async def test_copy_cli_requires_current_union_sha(copy_case, tmp_path):
    import json
    import subprocess
    import sys
    from pathlib import Path

    manifest, _, _, _ = copy_case
    path = tmp_path / 'manifest.json'
    path.write_text(json.dumps(manifest))
    path.chmod(0o600)
    root = Path(__file__).resolve().parents[2]
    current = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()
    for sha, expected in ((manifest['target_base_sha'], 1), (current, 0)):
        result = subprocess.run([sys.executable, '-m', 'scripts.phase0.copy',
            '--manifest', str(path), '--source-environment-file', str(tmp_path / 'absent-source'),
            '--target-environment-file', str(tmp_path / 'absent-target'),
            '--source-sha', manifest['source_sha'], '--target-sha', sha],
            cwd=root, capture_output=True, text=True)
        assert result.returncode == expected, result.stderr
        if expected == 0:
            assert 'no connections opened' in result.stdout

from tests.phase0.synthetic import uid
from tests.phase0.test_copy import copy_case as copy_case


@pytest.mark.asyncio
async def test_union_biofuel_histories_and_multibatch_cycles(copy_case):
    manifest, source, target, _ = copy_case
    # Use a separate owner connection only to prepare synthetic source fixtures.
    from sqlalchemy.ext.asyncio import create_async_engine
    engine = create_async_engine(source.engine.url.set(username='phase0_owner'))
    try:
        async with engine.begin() as owner:
            for number in range(257):
                parameters = {'rfq': uid(f'batch-rfq-{number}'), 'quote': uid(f'batch-quote-{number}'),
                              'buyer': uid('real-buyer'), 'seller': uid('real-seller'),
                              'trade': uid('snapshot-trade')}
                await owner.execute(text("""
                    INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, quantity_mt, status, expires_at)
                    VALUES (:rfq, :buyer, '9510c713-6e39-5080-add3-0a7c29657b79',
                            '73835e92-820e-584b-8280-bb61c63aa28e', 10, 'OPEN', '2026-01-15')
                """), parameters)
                await owner.execute(text("""
                    INSERT INTO rfq_quotes (id, rfq_id, seller_org_id, price_per_mt_usd, status)
                    VALUES (:quote, :rfq, :seller, 700, 'ACCEPTED')
                """), parameters)
                await owner.execute(text("""
                    UPDATE rfqs SET status='ACCEPTED', accepted_quote_id=:quote, trade_id=:trade WHERE id=:rfq
                """), parameters)
    finally:
        await engine.dispose()
    receipt = await copy_database(manifest, source, target)
    assert receipt.verification.tables['rfqs']['count'] == 258
    assert receipt.verification.tables['rfq_quotes']['count'] == 258
    assert (await target.execute(text("""
        SELECT count(*) FROM rfqs r JOIN rfq_quotes q ON r.accepted_quote_id=q.id AND q.rfq_id=r.id
    """))).scalar_one() == 258
    for product in ('B30', 'B100'):
        # New INSERTs must pass the restored union guard, not an import bypass.
        async with target.begin_nested() as probe:
            await target.execute(text("""
                INSERT INTO trades (id, buyer_id, seller_id, initiator_org_id, buyer_provenance,
                    seller_provenance, initiated_by, quantity_mt, price_per_mt_usd, status,
                    confirmed_at, product_id, product_name, fuel_type, fuel_grade, market_product,
                    delivery_point_id, delivery_point_name, delivery_point_region,
                    availability_window, market_snapshot_version)
                SELECT gen_random_uuid(), buyer_id, seller_id, initiator_org_id, buyer_provenance,
                    seller_provenance, initiated_by, quantity_mt, price_per_mt_usd, status,
                    confirmed_at, product_id, product_name, fuel_type, fuel_grade, market_product,
                    delivery_point_id, delivery_point_name, delivery_point_region,
                    availability_window, market_snapshot_version FROM trades WHERE id=:id
            """), {'id': uid(product + '-trade')})
            await probe.rollback()
        assert (await target.execute(text('SELECT market_product FROM trades WHERE id=:id'),
                                     {'id': uid(product + '-trade')})).scalar_one() == product
        assert (await target.execute(text("SELECT response_body->>'product' FROM market_command_results WHERE id=:id"),
                                     {'id': uid(product + '-receipt')})).scalar_one() == product
