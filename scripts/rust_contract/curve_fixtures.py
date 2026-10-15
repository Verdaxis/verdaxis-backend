"""Attested Task 10b fixtures shared by golden recording and differential probes."""
from datetime import datetime, timedelta
from decimal import Decimal
from uuid import UUID

from sqlalchemy import text


async def seed_curve_fixtures(session):
    import json
    import os
    from pathlib import Path

    from app.models.forward_monitoring import (
        FairPriceBand,
        MarketIndication,
        MarketSignalIngestionRun,
        PhysicalStem,
    )
    from scripts.rust_contract.seed_corpus import attest_session, matrix_module

    await attest_session(session)
    matrix = matrix_module()
    columns = list((await session.execute(text(
        "SELECT column_name FROM information_schema.columns "
        "WHERE table_schema='public' AND table_name='orderbook_orders' "
        "AND column_name <> 'acceptance_ordinal' ORDER BY ordinal_position"
    ))).scalars())
    source = matrix.fid('order:real_scalar_bid')
    names = ','.join(f'"{column}"' for column in columns)
    # The many-order workload is separate from the default table horizon.
    for label, window, count, price in [
        ('depth', '2031-02', 12, '1000+n'),
        ('depth-ask', '2031-02', 12, '1100+n'),
        ('bulk', '2031-04', 10000, '1000.000000000000000000000000000000::numeric'),
        ('older', '2031-05', 1, '1000'),
        ('newer', '2031-06', 1, '1000'),
        ('older-ask', '2031-05', 1, '1100'),
        ('newer-ask', '2031-06', 1, '1100'),
    ]:
        values = []
        for column in columns:
            if column == 'id':
                value = "md5(:label || n::text)::uuid"
            elif column == 'availability_window':
                value = ':window'
            elif column == 'price_per_mt_usd':
                value = price
            elif column in ('idempotency_key', 'idempotency_operation', 'idempotency_request_hash'):
                value = 'NULL'
            elif column in ('created_at', 'updated_at') and label.startswith(('older', 'newer')):
                value = 'CAST(:observed AS timestamptz)'
            else:
                value = f'o."{column}"'
            values.append(value)
        now = datetime.fromisoformat(os.environ['RUST_CONTRACT_CLOCK'])
        observed = now.replace(microsecond=0) - timedelta(hours=1)
        if label.startswith('newer'):
            observed += timedelta(microseconds=500000)
        await session.execute(text(
            f'INSERT INTO orderbook_orders ({names}) SELECT '+','.join(values)+
            ' FROM orderbook_orders o CROSS JOIN generate_series(1,:count) n '
            'WHERE o.id=:source ON CONFLICT (id) DO NOTHING'
        ), dict(label='curve10b:'+label+':', window=window, count=count,
                source=matrix.fid('order:ask_ok') if label.endswith('-ask') else source,
                observed=observed))
    row = (await session.execute(text(
        'SELECT product_id::text, delivery_point_id::text FROM orderbook_orders WHERE id=:id'
    ), {'id': source})).one()
    policy = json.loads(Path('deploy/rust/market-policy.json').read_text())
    code = next(p['code'] for p in policy['products'] if p['id'] == row[0])
    for family, model, kind in [
        ('MARKET_INDICATION', MarketIndication, 'MARKET_INDICATION'),
        ('FAIR_PRICE_BAND', FairPriceBand, 'FAIR_PRICE_MODEL'),
        ('PHYSICAL_STEM', PhysicalStem, 'PHYSICAL_STEM'),
    ]:
        run = MarketSignalIngestionRun(
            id=matrix.fid('curve10b:run:'+family), signal_family=family,
            source='CURVE10B', source_kind=kind, started_at=now,
            verified_at=now, created_at=now,
        )
        if await session.get(MarketSignalIngestionRun, run.id) is not None:
            continue
        session.add(run)
        await session.flush()
        for evidence in ('REAL', 'DEMO', 'UNKNOWN'):
            for index in range(12 if family == 'PHYSICAL_STEM' else 1):
                values = dict(
                    id=matrix.fid(f'curve10b:signal:{family}:{evidence}:{index}'),
                    market_product=code, delivery_point_id=UUID(row[1]),
                    availability_window='2031-03', source='CURVE10B',
                    is_demo=evidence == 'DEMO', is_verified_real=evidence == 'REAL',
                    trusted_ingestion_run_id=run.id if evidence == 'REAL' else None,
                    observed_at=now-timedelta(hours=2 if evidence == 'REAL' else 1, minutes=index),
                    created_at=now,
                )
                if family == 'MARKET_INDICATION':
                    values.update(side='MID', price_per_mt_usd=Decimal('123.40'), quantity_mt=Decimal('12.30'))
                elif family == 'FAIR_PRICE_BAND':
                    values.update(low_price_per_mt_usd=Decimal('100.10'), mid_price_per_mt_usd=Decimal('120.20'), high_price_per_mt_usd=Decimal('130.30'), model_name='fixture')
                else:
                    values.update(stem_uid=f'{evidence}:{index}', quantity_mt=Decimal('12.30'), status='AVAILABLE', stem_start=now, stem_end=now+timedelta(days=1))
                session.add(model(**values))
    await session.flush()
    return row
