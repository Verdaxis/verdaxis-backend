"""Task 10b parity probes against the harness-attested populated corpus."""
import asyncio
import importlib.util
import json
import os
import sys
from pathlib import Path
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from scripts.rust_contract.record import differences, record


async def depth_fixture():
    from sqlalchemy import text

    from app.seeds.safety import seed_session
    spec = importlib.util.spec_from_file_location('matrix', Path(__file__).with_name('market_contract_oracle.py'))
    matrix = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(matrix)
    async with seed_session() as session:
        columns = (await session.execute(text("SELECT column_name FROM information_schema.columns WHERE table_schema='public' AND table_name='orderbook_orders' ORDER BY ordinal_position"))).scalars().all()
        columns.remove('acceptance_ordinal')
        for index in range(12):
            projections = ["CAST(:id AS uuid)" if c == 'id' else "'2031-02'" if c == 'availability_window' else ':price' if c == 'price_per_mt_usd' else 'NULL' if c in ('idempotency_key','idempotency_operation','idempotency_request_hash') else f'"{c}"' for c in columns]
            await session.execute(text('INSERT INTO orderbook_orders ('+','.join(f'"{c}"' for c in columns)+') SELECT '+','.join(projections)+' FROM orderbook_orders WHERE id=:source'),
                                  {'id':matrix.fid(f'curve10b:depth:{index}'),'source':matrix.fid('order:real_scalar_bid'),'price':1000+index})
        row = (await session.execute(text('SELECT product_id::text, delivery_point_id::text FROM orderbook_orders WHERE id=:id'), {'id':matrix.fid('order:real_scalar_bid')})).one()
        from datetime import datetime, timedelta
        from decimal import Decimal
        from uuid import UUID

        from app.models.forward_monitoring import (
            FairPriceBand,
            MarketIndication,
            MarketSignalIngestionRun,
            PhysicalStem,
        )
        now = datetime.fromisoformat(os.environ['RUST_CONTRACT_CLOCK'])
        policy = json.loads(Path('deploy/rust/market-policy.json').read_text())
        code = next(p['code'] for p in policy['products'] if p['id'] == row[0])
        for family, model, kind in [('MARKET_INDICATION', MarketIndication, 'MARKET_INDICATION'), ('FAIR_PRICE_BAND', FairPriceBand, 'FAIR_PRICE_MODEL'), ('PHYSICAL_STEM', PhysicalStem, 'PHYSICAL_STEM')]:
            run = MarketSignalIngestionRun(id=matrix.fid('curve10b:run:'+family), signal_family=family, source='CURVE10B', source_kind=kind, started_at=now, verified_at=now, created_at=now)
            session.add(run)
            await session.flush()
            for evidence in ('REAL', 'DEMO', 'UNKNOWN'):
                for index in range(12 if family == 'PHYSICAL_STEM' else 1):
                    values = dict(id=matrix.fid(f'curve10b:signal:{family}:{evidence}:{index}'), market_product=code, delivery_point_id=UUID(row[1]), availability_window='2031-03', source='CURVE10B', is_demo=evidence=='DEMO', is_verified_real=evidence=='REAL', trusted_ingestion_run_id=run.id if evidence=='REAL' else None, observed_at=now-timedelta(hours=2 if evidence=='REAL' else 1, minutes=index), created_at=now)
                    if family == 'MARKET_INDICATION':
                        values.update(side='MID',price_per_mt_usd=Decimal('123.40'),quantity_mt=Decimal('12.30'))
                    elif family == 'FAIR_PRICE_BAND':
                        values.update(low_price_per_mt_usd=Decimal('100.10'),mid_price_per_mt_usd=Decimal('120.20'),high_price_per_mt_usd=Decimal('130.30'),model_name='fixture')
                    else:
                        values.update(stem_uid=f'{evidence}:{index}',quantity_mt=Decimal('12.30'),status='AVAILABLE',stem_start=now,stem_end=now+timedelta(days=1))
                    session.add(model(**values))
        await session.commit()
        return row


def main():
    product_id, depth_point = asyncio.run(depth_fixture())
    policy = json.loads(Path('deploy/rust/market-policy.json').read_text())
    curve_policy = json.loads(Path('rs/read/src/market/curve_policy.json').read_text())
    from app.market_catalog import MarketProduct
    from app.services.benchmarks import _base_benchmark_price
    assert curve_policy['source_sha'] == policy['source_sha']
    assert curve_policy['market_product_enum'] == [item.value for item in MarketProduct]
    expected_quotes = []
    for product in policy['products']:
        for point in policy['points']:
            price = _base_benchmark_price(product['code'], point['name'])
            if price is not None:
                expected_quotes.append(dict(market_product=product['code'], delivery_point_id=point['id'], base_price=str(price)))
    assert curve_policy['seed_quotes'] == expected_quotes
    depth_product = next(p['code'] for p in policy['products'] if p['id'] == product_id)
    cases = [dict(id='ten-level-depth', method='GET', path='/api/curves/forward/slice', query=urlencode(dict(market_product=depth_product, delivery_point_id=depth_point, availability_window='2031-02')))]
    for product in policy['products']:
        for point in product['lanes']:
            for window in ('SPOT', '2026-11', '2030-Q1'):
                cases.append(dict(id=f"slice-{len(cases)}", method='GET', path='/api/curves/forward/slice',
                                  query=urlencode(dict(market_product=product['code'], delivery_point_id=point, availability_window=window))))
    cases.append(dict(id='real-signals-before-limit', method='GET', path='/api/curves/forward/slice', query=urlencode(dict(market_product=depth_product, delivery_point_id=depth_point, availability_window='2031-03'))))
    cases.extend([
        dict(id='duplicates',method='GET',path='/api/curves/forward/table',query='windows=2026-11&windows=SPOT&windows=2026-11'),
        dict(id='unknown-query',method='GET',path='/api/curves/forward/table',query='shape=board&availability_window=bad&windows=SPOT'),
        dict(id='too-many-windows',method='GET',path='/api/curves/forward/table',query='&'.join(f'windows={year}-Q1' for year in range(2026,2051))),
        dict(id='unsupported-lane',method='GET',path='/api/curves/forward/slice',query=f"market_product={policy['products'][0]['code']}&delivery_point_id={depth_point}&availability_window=SPOT"),
        dict(id='bad-window',method='GET',path='/api/curves/forward/table',query='windows=Q1_2026'),
        dict(id='bad-product',method='GET',path='/api/curves/forward/slice',query='market_product=INVALID&delivery_point_id=00000000-0000-0000-0000-000000000000&availability_window=SPOT'),
        dict(id='unknown-point',method='GET',path='/api/curves/forward/slice',query=f"market_product={policy['products'][0]['code']}&delivery_point_id=00000000-0000-0000-0000-000000000000&availability_window=SPOT"),
        dict(id='fallback-point',method='GET',path='/api/curves/forward/board',query='focus_delivery_point_id=00000000-0000-0000-0000-000000000000'),
    ])
    token=os.environ['DISPOSABLE_TEST_TOKEN']
    failures=[]
    saw_unknown = False
    saw_depth_limit = False
    forbidden = ('organization_id', 'owner_user_id', 'created_by_actor_user_id', 'trusted_ingestion_run_id', 'certificate_number', 'physical_supplier_name')
    for case in cases:
        python=record(os.environ['PYTHON_CONTRACT_URL'],token,[case])
        rust=record(os.environ['RUST_CONTRACT_URL'],token,[case])
        problems=differences(python,rust)
        body = rust[0]['exchange']['body']
        wire = json.dumps(body)
        assert not any(f'"{field}":' in wire for field in forbidden), 'private field leaked'
        if case['path'].endswith('/slice') and rust[0]['exchange']['status'] == 200:
            saw_unknown |= body['cell']['unknown_order_count'] > 0
            saw_depth_limit |= len(body['depth_bids']) == 10 or len(body['depth_asks']) == 10
            assert len(body['depth_bids']) <= 10 and len(body['depth_asks']) <= 10
            assert len(body['trades']) <= 8 and len(body['indications']) <= 10
            assert len(body['physical_stems']) <= 6
            for items in (body['trades'], body['depth_bids'], body['depth_asks']):
                if items and items[0]['demo_status'] == 'REAL_ONLY':
                    assert all(item['demo_status'] == 'REAL_ONLY' for item in items)
        if case['id'] == 'real-signals-before-limit':
            assert len(body['physical_stems']) == 6
            assert all(stem['provenance']['demo_status'] == 'REAL_ONLY' for stem in body['physical_stems'])
            assert body['indications'][0]['price_per_mt_usd'] == '123.40'
            assert body['indications'][0]['quantity_mt'] == '12.30'
            assert body['fair_price_band']['mid_price_per_mt_usd'] == '120.20'
            assert body['cell']['indication_summary']['provenance']['unknown_count'] == 1
        if problems:
            Path('work/rust-phase1/task10b-probe-failure.json').write_text(json.dumps({'case':case,'python':python,'rust':rust},indent=2,default=str))
            failures.extend(problems)
            print('\n'.join(problems),flush=True)
            break
    assert not failures, failures
    assert saw_unknown, 'corpus UNKNOWN metadata was not exercised'
    assert saw_depth_limit, 'ten-level depth bound was not exercised'
    print(f'Canonical curve probes: {len(cases)} matched; UNKNOWN covered; ten-level depth={saw_depth_limit}',flush=True)


if __name__=='__main__':
    main()
