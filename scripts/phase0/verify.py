"""Read-only copy verification. Reports contain hashes and counts, not rows."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, localcontext
from uuid import UUID

from sqlalchemy import text

from scripts.phase0.codecs import RowHash, quote, relation, text_batches


@dataclass(frozen=True)
class VerificationReport:
    tables: dict
    numeric_totals: dict
    new_reference_rows: dict
    notice_count: int


async def sequence_states(connection, catalog):
    states = {}
    for sequence in catalog['sequences']:
        row = (await connection.execute(text(
            f'SELECT last_value, is_called FROM {relation(sequence["name"])}'
        ))).mappings().one()
        states[sequence['name']] = dict(row)
    return states


async def table_hash(connection, table, columns=None):
    names = columns or tuple(column['name'] for column in table['columns'])
    result = RowHash(names)
    async for batch in text_batches(connection, table, names):
        for row in batch:
            result.add(row)
    return result.report()


async def fingerprint_database(connection, catalog):
    await connection.execute(text("SET LOCAL TIME ZONE 'UTC'"))
    tables = {name: await table_hash(connection, table) for name, table in catalog['tables'].items()}
    return dict(tables=tables, sequences=await sequence_states(connection, catalog))


async def notice_receipts(connection):
    rows = (await connection.execute(text("""
        SELECT resource_id, min(timestamp) AS sent_at
        FROM audit_logs WHERE action='kyc.grace_notice_sent'
          AND resource_type='kyc_execution_grace'
        GROUP BY resource_id ORDER BY resource_id
    """))).mappings()
    result = {}
    for row in rows:
        try:
            identity = UUID(row['resource_id'])
        except (ValueError, TypeError, AttributeError):
            raise ValueError('Malformed notice evidence') from None
        if str(identity) != row['resource_id'] or row['sent_at'] is None:
            raise ValueError('Malformed notice evidence')
        exists = (await connection.execute(text('SELECT count(*) FROM kyc_execution_grace WHERE id=:id'), {'id': identity})).scalar_one()
        if exists != 1:
            raise ValueError('Orphan notice evidence')
        result[identity] = row['sent_at']
    return result


async def check_historical_invariants(connection):
    checks = {
        'negotiations': "product_id='8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid",
        'inventory_items': "fuel_type::text IN ('LNG','BIO_LNG')",
        'market_support_authorizations': "product_id='8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid",
        'trades': "market_snapshot_version IS NOT NULL AND market_snapshot_version <> 1",
        'kyc_execution_grace': "kyc_status_at_enrollment NOT IN ('PENDING','APPROVED') OR expires_at <> enrolled_at + interval '14 days'",
        'market_event_outbox': 'stream_seq IS NULL OR dispatched_at IS NULL',
    }
    for table in ('market_indications', 'fair_price_bands', 'physical_stems', 'benchmarks', 'live_slice_benchmarks'):
        checks[table] = "market_product='BIO_LNG' AND delivery_point_id IS DISTINCT FROM '1379d36c-1ca9-55b7-9c0d-5235a0ba1f36'::uuid"
    for table, condition in checks.items():
        count = (await connection.execute(text(f'SELECT count(*) FROM {relation(table)} WHERE {condition}'))).scalar_one()
        if count:
            raise ValueError(f'Historical invariant failed: {table}; count={count}')


async def verify_database(manifest, source, target) -> VerificationReport:
    tables, totals, new_reference = {}, {}, {}
    for name, mapping in manifest['tables'].items():
        if mapping['classification'] == 'skip':
            continue
        source_table = manifest['source_catalog']['tables'][name]
        target_table = manifest['target_catalog']['tables'][name]
        columns = tuple(mapping['columns'])
        source_hash, target_hash = RowHash(columns), RowHash(columns)
        source_totals = {c['name']: Decimal(0) for c in source_table['columns'] if c['type'].startswith('numeric')}
        target_totals = source_totals.copy()
        if mapping['classification'] == 'regenerate':
            # Catalogs are bounded reference data; business tables always stream.
            source_rows = {}
            async for batch in text_batches(source, source_table, columns):
                for row in batch:
                    source_rows[row['id']] = row
            extra = RowHash(columns)
            async for batch in text_batches(target, target_table, columns):
                for row in batch:
                    if row['id'] in source_rows:
                        if row != source_rows.pop(row['id']):
                            raise ValueError(f'Reference values differ: {name}')
                        source_hash.add(row)
                        target_hash.add(row)
                    else:
                        baseline = manifest['execution']['baseline_reference_rows'][name]
                        if row not in baseline:
                            raise ValueError(f'Unexpected reference row: {name}')
                        extra.add(row)
            if source_rows:
                raise ValueError(f'Missing source reference rows: {name}')
            new_reference[name] = extra.report()
        else:
            for connection, table, hasher, numeric in ((source, source_table, source_hash, source_totals), (target, target_table, target_hash, target_totals)):
                async for batch in text_batches(connection, table, columns):
                    for row in batch:
                        hasher.add(row)
                        for column in numeric:
                            if row[column] is not None:
                                value = Decimal(row[column])
                                # PostgreSQL numeric permits up to 131072 digits
                                # before and 16383 after the decimal point.
                                with localcontext() as context:
                                    context.prec = 150000
                                    numeric[column] += value
            if source_totals != target_totals:
                raise ValueError(f'Decimal totals differ: {name}')
        if source_hash.report() != target_hash.report():
            raise ValueError(f'Copied values differ: {name}')
        tables[name] = target_hash.report()
        totals[name] = {column: str(value) for column, value in target_totals.items()}
        for column, addition in mapping['target_additions'].items():
            expected = addition['value']
            condition = f'{quote(column)} IS NOT NULL' if expected is None else f'{quote(column)} IS DISTINCT FROM :expected'
            count = (await target.execute(text(f'SELECT count(*) FROM {relation(name)} WHERE {condition}'), {} if expected is None else {'expected': expected})).scalar_one()
            if count:
                raise ValueError(f'Target mapping differs: {name}')
    for name in ('refresh_sessions', 'supplier_offers', 'kyc_documents'):
        if (await target.execute(text(f'SELECT count(*) FROM {relation(name)}'))).scalar_one():
            raise ValueError(f'Table must remain empty: {name}')
    expected = await notice_receipts(source)
    actual = (await target.execute(text('SELECT grace_id, claimed_at, sent_at FROM kyc_grace_notices'))).mappings().all()
    if len(actual) != len(expected) or any(row['claimed_at'] != expected.get(row['grace_id']) or row['sent_at'] != row['claimed_at'] for row in actual):
        raise ValueError('Notice receipt mapping differs')
    await check_historical_invariants(target)
    return VerificationReport(tables, totals, new_reference, len(expected))
