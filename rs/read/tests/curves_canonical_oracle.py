"""Task 10b parity probes against the harness-attested populated corpus."""
import asyncio
import json
import os
import sys
from pathlib import Path
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from scripts.rust_contract.record import differences, record


async def depth_fixture():
    from app.seeds.safety import seed_session
    from scripts.rust_contract.curve_fixtures import seed_curve_fixtures
    async with seed_session() as session:
        row = await seed_curve_fixtures(session)
        await session.commit()
        return row


def measure_requests(product, point):
    import concurrent.futures
    import subprocess
    import threading

    token = os.environ['DISPOSABLE_TEST_TOKEN']
    metrics = Path('work/rust-phase1/curve-query-metrics.jsonl')
    measurements = []
    cases = [
        ('large-cell', 'table', dict(market_products=product, delivery_point_ids=point, windows='2031-04'), 8),
        ('large-depth', 'slice', dict(market_product=product, delivery_point_id=point, availability_window='2031-04'), 10),
        ('matrix', 'table', {}, 8),
        ('board', 'board', dict(focus_market_product=product, focus_delivery_point_id=point), 9),
    ]
    for name, shape, query, expected in cases:
        case = dict(id=name, method='GET', path='/api/curves/forward/'+shape, query=urlencode(query))
        python = record(os.environ['PYTHON_CONTRACT_URL'], token, [case])
        before = len(metrics.read_text().splitlines())
        rust = record(os.environ['RUST_CONTRACT_URL'], token, [case])
        assert not differences(python, rust), differences(python, rust)
        events = [json.loads(line) for line in metrics.read_text().splitlines()[before:]]
        data = [event for event in events if not event['ping'] and not event.get('control', False)]
        result = dict(case=name, statements=len(data), control_statements=sum(e.get('control', False) for e in events), rows=sum(e['rows'] for e in data), events=events)
        measurements.append(result)
        print('CURVE_QUERY_COST', json.dumps(result), flush=True)
        if name.startswith('large'):
            # Two catalog queries return the active products and points. The
            # economic input is one group; depth adds exactly one price level.
            policy = json.loads(Path('deploy/rust/market-policy.json').read_text())
            catalog_rows = sum(p.get('execution_mode') == 'ORDERBOOK' for p in policy['products']) + len(policy['points'])
            # The fixture intentionally has one inactive product and point.
            catalog_rows -= 2
            expected_rows = catalog_rows + (2 if name == 'large-depth' else 1)
            assert result['rows'] == expected_rows, 'raw orders crossed the wire instead of grouped rows'
        assert result['statements'] == expected, result
    pid = int(Path('work/rust-phase1/curve-server.pid').read_text())
    def rss():
        return int(subprocess.check_output(['ps', '-o', 'rss=', '-p', str(pid)], text=True).strip())
    baseline = rss()
    samples = [baseline]
    stop = threading.Event()
    def sample():
        while not stop.wait(0.01):
            samples.append(rss())
    monitor = threading.Thread(target=sample)
    monitor.start()
    case = dict(id='concurrent-large-cell', method='GET', path='/api/curves/forward/table', query=urlencode(dict(market_products=product, delivery_point_ids=point, windows='2031-04')))
    barrier = threading.Barrier(8)
    def request(_):
        barrier.wait()
        result = record(os.environ['RUST_CONTRACT_URL'], token, [case])
        assert result[0]['exchange']['status'] == 200
    try:
        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
            list(executor.map(request, range(8)))
    finally:
        stop.set()
        monitor.join()
    memory = dict(concurrency=8, orders=10000, rss_before_kib=baseline, rss_peak_kib=max(samples), rss_after_kib=rss(), samples=len(samples))
    Path('work/rust-phase1/task10b-query-cost.json').write_text(json.dumps(dict(requests=measurements, memory=memory), indent=2)+'\n')
    print('CURVE_MEMORY', json.dumps(memory), flush=True)


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
    measure_requests(depth_product, depth_point)
    subsecond = dict(id='subsecond-latest', method='GET', path='/api/curves/forward/table',
                     query=urlencode(dict(market_products=depth_product, delivery_point_ids=depth_point,
                                          windows=['2031-05', '2031-06']), doseq=True))
    subsecond_body = record(os.environ['RUST_CONTRACT_URL'], os.environ['DISPOSABLE_TEST_TOKEN'], [subsecond])[0]['exchange']['body']
    assert [item['availability_window'] for item in subsecond_body['latest_signals']] == ['2031-06', '2031-05']
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
        if case['id'] == 'ten-level-depth':
            assert len(body['depth_bids']) == len(body['depth_asks']) == 10
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
