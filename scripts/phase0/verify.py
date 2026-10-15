"""Read-only copy verification. Reports contain hashes and counts, not rows."""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, localcontext
from uuid import UUID

from sqlalchemy import text

from scripts.phase0.codecs import (
    RowHash,
    configure_text_session,
    quote,
    relation,
    text_batches,
)


@dataclass(frozen=True)
class VerificationReport:
    tables: dict
    numeric_totals: dict
    new_reference_rows: dict
    notice_count: int


async def require_unfiltered_reads(connection):
    # This one-use contract permits no RLS, even owner-bypassed or FORCE-only
    # drift. row_security=off makes a later filtered read fail, not bypass RLS.
    await connection.execute(text('SET LOCAL row_security=off'))
    bypass = (await connection.execute(text(
        'SELECT rolbypassrls FROM pg_roles WHERE rolname=current_user'
    ))).scalar_one()
    secured = (await connection.execute(text("""
        SELECT EXISTS (
            SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE c.relkind IN ('r','p')
              AND n.nspname NOT IN ('pg_catalog','information_schema')
              AND n.nspname NOT LIKE 'pg_toast%'
              AND (c.relrowsecurity OR c.relforcerowsecurity)
              AND NOT EXISTS (
                  SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass
                    AND d.objid=c.oid AND d.deptype='e'
              )
        )
    """))).scalar_one()
    if bypass or secured:
        raise ValueError('Row-level security or bypass authority is not approved')


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
    await configure_text_session(connection)
    await require_unfiltered_reads(connection)
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
    await require_unfiltered_reads(source)
    await require_unfiltered_reads(target)
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
            expected_extras = {
                row['id']: row
                for row in manifest['execution']['baseline_reference_rows'][name]
                if row['id'] not in source_rows
            }
            extra = RowHash(columns)
            async for batch in text_batches(target, target_table, columns):
                for row in batch:
                    if row['id'] in source_rows:
                        if row != source_rows.pop(row['id']):
                            raise ValueError(f'Reference values differ: {name}')
                        source_hash.add(row)
                        target_hash.add(row)
                    else:
                        if row != expected_extras.pop(row['id'], None):
                            raise ValueError(f'Unexpected reference row: {name}')
                        extra.add(row)
            if source_rows:
                raise ValueError(f'Missing source reference rows: {name}')
            if expected_extras:
                raise ValueError(f'Missing baseline reference rows: {name}')
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


async def verify_sequences(manifest, source, target):
    from scripts.phase0.codecs import next_sequence_state
    from scripts.phase0.copy import GLOBAL_SEQUENCES

    source_states = await sequence_states(source, manifest['source_catalog'])
    target_states = await sequence_states(target, manifest['target_catalog'])
    if manifest['source_catalog']['sequences'] != manifest['target_catalog']['sequences']:
        raise ValueError('Sequence definitions differ')
    results = {}
    for definition in manifest['target_catalog']['sequences']:
        name = definition['name']
        if definition['owner_table']:
            table, column = definition['owner_table'], definition['owner_column']
        else:
            table, column = GLOBAL_SEQUENCES[name]
        maximum = (await target.execute(text(
            f'SELECT max({quote(column)}) FROM {relation(table)}'
        ))).scalar_one()
        state = source_states[name]
        value, called, next_value = next_sequence_state(
            definition, state['last_value'], state['is_called'], maximum,
        )
        if target_states[name] != dict(last_value=value, is_called=called):
            raise ValueError('Target sequence state differs')
        results[name] = dict(source=state, last_value=value, is_called=called, next_value=next_value)
    return results


async def verify_independent(manifest, source, target):
    """Verify committed storage without a copy receipt or any write transaction."""
    from dataclasses import asdict

    from scripts.phase0.copy import attest, validate_manifest
    from scripts.phase0.manifest import (
        digest,
        inspect_catalog,
        require_complete_inventory,
    )

    validate_manifest(manifest)
    execution = manifest['execution']
    if execution['source_catalog_sha256'] != digest(manifest['source_catalog']):
        raise ValueError('Frozen source catalog binding differs')
    frozen = execution['frozen_source']
    if source.in_transaction() or target.in_transaction():
        raise ValueError('Verification owns independent transactions')
    async with source.begin(), target.begin():
        for side, connection in (('source', source), ('target', target)):
            await connection.execute(text('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY'))
            await configure_text_session(connection)
            await connection.execute(text('SET LOCAL search_path=public,pg_catalog'))
            await attest(connection, execution[side], owner=side == 'target')
            cluster = (await connection.execute(text(
                'SELECT system_identifier::text FROM pg_control_system()'
            ))).scalar_one()
            if cluster != execution[side]['cluster_system_identifier']:
                raise ValueError('Connected cluster differs')
            require_complete_inventory(await connection.run_sync(inspect_catalog), manifest[f'{side}_catalog'])
        source_fingerprint = await fingerprint_database(source, manifest['source_catalog'])
        if source_fingerprint != frozen:
            raise ValueError('Frozen source changed')
        verification = await verify_database(manifest, source, target)
        sequences = await verify_sequences(manifest, source, target)
        # Sequences are not MVCC snapshots: a late sequence writer must refuse.
        if await sequence_states(source, manifest['source_catalog']) != frozen['sequences']:
            raise ValueError('Frozen source sequence changed')
        return dict(manifest_digest=digest(manifest), source_fingerprint=source_fingerprint,
                    verification=asdict(verification), sequences=sequences,
                    source_identity=execution['source'], target_identity=execution['target'])


def main():
    import argparse
    import asyncio
    import json
    import os
    from pathlib import Path

    from sqlalchemy.ext.asyncio import create_async_engine
    from sqlalchemy.pool import NullPool

    from scripts.phase0.copy import (
        private_json,
        read_private_file,
        validate_manifest,
        validate_url,
    )

    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--source-environment-file', type=Path, required=True)
    parser.add_argument('--target-environment-file', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    try:
        if os.path.lexists(args.report):
            raise ValueError('Report destination already exists')
        manifest = json.loads(read_private_file(args.manifest))
        validate_manifest(manifest)
        # Require the freeze before opening a connection, including after receipt loss.
        if not manifest['execution']['frozen_source'] or not manifest['execution']['source_catalog_sha256']:
            raise ValueError('Frozen source evidence required')
        urls = []
        for path in (args.source_environment_file, args.target_environment_file):
            lines = read_private_file(path).splitlines()
            if len(lines) != 1 or not lines[0].startswith('DATABASE_URL='):
                raise ValueError('Expected one explicit DATABASE_URL entry')
            urls.append(validate_url(lines[0].split('=', 1)[1], manifest['execution']))

        async def run():
            engines = [create_async_engine(url, hide_parameters=True, poolclass=NullPool,
                       connect_args={'server_settings': {'default_transaction_read_only': 'on'}})
                       for url in urls]
            try:
                async with engines[0].connect() as source, engines[1].connect() as target:
                    return await verify_independent(manifest, source, target)
            finally:
                for engine in engines:
                    await engine.dispose()

        report = asyncio.run(run())
        private_json(args.report, report)
    except Exception:
        parser.exit(1, 'Verification refused; no successful report published\n')
    print('Committed copy verified; private report published')


if __name__ == '__main__':
    main()
