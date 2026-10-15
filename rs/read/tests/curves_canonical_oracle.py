"""Task 10b parity probes against the harness-attested populated corpus."""
import json
import os
import sys
from pathlib import Path
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parents[3]))
from scripts.rust_contract.record import differences, record


def main():
    policy = json.loads(Path('deploy/rust/market-policy.json').read_text())
    cases = []
    for product in policy['products']:
        for point in product['lanes']:
            for window in ('SPOT', '2026-11', '2030-Q1'):
                cases.append(dict(id=f"slice-{len(cases)}", method='GET', path='/api/curves/forward/slice',
                                  query=urlencode(dict(market_product=product['code'], delivery_point_id=point, availability_window=window))))
    cases.extend([
        dict(id='duplicates',method='GET',path='/api/curves/forward/table',query='windows=2026-11&windows=SPOT&windows=2026-11'),
        dict(id='bad-window',method='GET',path='/api/curves/forward/table',query='windows=Q1_2026'),
        dict(id='bad-product',method='GET',path='/api/curves/forward/slice',query='market_product=INVALID&delivery_point_id=00000000-0000-0000-0000-000000000000&availability_window=SPOT'),
        dict(id='unknown-point',method='GET',path='/api/curves/forward/slice',query=f"market_product={policy['products'][0]['code']}&delivery_point_id=00000000-0000-0000-0000-000000000000&availability_window=SPOT"),
        dict(id='fallback-point',method='GET',path='/api/curves/forward/board',query='focus_delivery_point_id=00000000-0000-0000-0000-000000000000'),
    ])
    token=os.environ['DISPOSABLE_TEST_TOKEN']
    failures=[]
    saw_unknown = False
    saw_depth_limit = False
    for case in cases:
        python=record(os.environ['PYTHON_CONTRACT_URL'],token,[case])
        rust=record(os.environ['RUST_CONTRACT_URL'],token,[case])
        problems=differences(python,rust)
        body = rust[0]['exchange']['body']
        if case['path'].endswith('/slice') and rust[0]['exchange']['status'] == 200:
            saw_unknown |= body['cell']['unknown_order_count'] > 0
            saw_depth_limit |= len(body['depth_bids']) == 10 or len(body['depth_asks']) == 10
            assert len(body['depth_bids']) <= 10 and len(body['depth_asks']) <= 10
            assert len(body['trades']) <= 8 and len(body['indications']) <= 10
            assert len(body['physical_stems']) <= 6
            for items in (body['trades'], body['depth_bids'], body['depth_asks']):
                if items and items[0]['demo_status'] == 'REAL_ONLY':
                    assert all(item['demo_status'] == 'REAL_ONLY' for item in items)
        if problems:
            Path('work/rust-phase1/task10b-probe-failure.json').write_text(json.dumps({'case':case,'python':python,'rust':rust},indent=2,default=str))
            failures.extend(problems)
            print('\n'.join(problems),flush=True)
            break
    assert not failures, failures
    assert saw_unknown, 'corpus UNKNOWN metadata was not exercised'
    print(f'Canonical curve probes: {len(cases)} matched; UNKNOWN covered; ten-level depth={saw_depth_limit}',flush=True)


if __name__=='__main__':
    main()
