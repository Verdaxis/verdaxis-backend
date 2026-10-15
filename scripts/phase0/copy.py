"""One-use, host-local Phase 0 import. No runtime grants or replay mode.

The provisional contract permits synthetic proof only. Host apply stays closed
until the union revision and its canonical ACL bundle receive final review.
"""
from __future__ import annotations

import argparse
import asyncio
import ipaddress
import json
import os
import re
import stat
from dataclasses import asdict, dataclass
from pathlib import Path

from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine

from scripts.phase0.codecs import (
    TableMap,
    configure_text_session,
    insert_statement,
    map_row,
    next_sequence_state,
    quote,
    relation,
    text_batches,
)
from scripts.phase0.manifest import (
    digest,
    inspect_catalog,
    load_manifest,
    require_complete_inventory,
)
from scripts.phase0.verify import (
    VerificationReport,
    check_historical_invariants,
    notice_receipts,
    require_unfiltered_reads,
    sequence_states,
    verify_database,
)

# Only these edges are cyclic in the reviewed catalogs. Self-references need
# deferral too because referenced users can occur in a later bounded batch.
CYCLIC_FKS = {
    ('rfqs', 'fk_rfqs_accepted_quote_id'),
    ('rfq_quotes', 'rfq_quotes_rfq_id_fkey'),
    ('users', 'fk_users_kyc_reviewed_by'),
    ('users', 'users_referred_by_id_fkey'),
}
GLOBAL_SEQUENCES = {
    'orderbook_acceptance_ordinal_seq': ('orderbook_orders', 'acceptance_ordinal'),
    'market_event_stream_seq': ('market_event_outbox', 'stream_seq'),
}
IMPORT_LOCK = 706016005


class CopyRefused(ValueError):
    """Safe public failure; SQL detail belongs only in the private report."""


@dataclass(frozen=True)
class CopyReceipt:
    manifest_digest: str
    source_snapshot: str
    target_oid: int
    verification: VerificationReport
    sequences: dict


def read_private_file(path, *, root_owned=False):
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode) != 0o600:
                raise CopyRefused('Private regular mode-0600 file required')
            if info.st_uid != (0 if root_owned else os.geteuid()) or info.st_size > 8_000_000:
                raise CopyRefused('Private file ownership or size refused')
            with os.fdopen(descriptor, 'r', closefd=False) as stream:
                return stream.read()
        finally:
            os.close(descriptor)
    except OSError:
        raise CopyRefused('Private file could not be opened') from None


def private_json(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'w') as stream:
        json.dump(value, stream, sort_keys=True, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    parent = os.open(Path(path).parent, os.O_RDONLY)
    try:
        os.fsync(parent)
    finally:
        os.close(parent)


def validate_url(raw, execution):
    try:
        url = make_url(raw)
        if url.drivername not in ('postgresql', 'postgresql+asyncpg') or url.query:
            raise ValueError
        if not ipaddress.ip_address(url.host).is_loopback or not url.password or not url.username:
            raise ValueError
        if not url.port or not 1024 < url.port < 65536:
            raise ValueError
        if execution['mode'] == 'synthetic' and not (url.database or '').startswith('phase0_'):
            raise ValueError
        return url.set(drivername='postgresql+asyncpg')
    except Exception:
        raise CopyRefused('Database URL refused') from None


def validate_manifest(manifest):
    committed = load_manifest()
    union_path = Path(__file__).resolve().parents[2] / 'deploy/phase0/provisional-union.json'
    union = json.loads(union_path.read_text())
    union_values = {key: value for key, value in union.items() if key != 'provisional_digest'}
    if digest(union_values) != union['provisional_digest']:
        raise CopyRefused('Provisional union digest differs')
    if union['source_sha'] != committed['source_sha'] or union['target_base_sha'] != committed['target_base_sha']:
        raise CopyRefused('Provisional union source pins differ')
    if {k: v for k, v in manifest.items() if k != 'execution'} != committed:
        raise CopyRefused('Copy contract differs from committed review')
    execution = manifest['execution']
    if execution['mode'] != 'synthetic':
        raise CopyRefused('Host apply awaits Task 4b union and ACL acceptance')
    if execution['host_id'] != os.uname().nodename:
        raise CopyRefused('Host identity differs')
    identities = []
    for side in ('source', 'target'):
        identity = execution[side]
        if not ipaddress.ip_address(identity['host']).is_loopback:
            raise CopyRefused('Nonlocal endpoint refused')
        if identity['port'] != int(os.environ.get('PHASE0_PORT', '0')):
            raise CopyRefused('Disposable runner endpoint required')
        if not identity['database'].startswith('phase0_'):
            raise CopyRefused('Synthetic database name required')
        if (not str(identity['cluster_system_identifier']).isdigit()
                or str(identity['cluster_system_identifier']) != os.environ.get('PHASE0_CLUSTER_ID')):
            raise CopyRefused('Trusted cluster binding required')
        identities.append(tuple(identity[k] for k in ('cluster_system_identifier', 'host', 'port', 'database', 'oid')))
    if identities[0] == identities[1]:
        raise CopyRefused('Source and target identities must differ')
    if not execution['source']['role'].startswith('phase0_backup_') or execution['target']['role'] != 'phase0_owner':
        raise CopyRefused('Separate backup and owner roles required')


async def attest(connection, identity, *, owner):
    url = connection.engine.url
    if (url.host, url.port, url.database, url.username) != tuple(identity[k] for k in ('host', 'port', 'database', 'role')):
        raise CopyRefused('Connection endpoint differs')
    row = (await connection.execute(text("""
        SELECT current_database() AS database, current_user AS role, d.oid,
               r.rolsuper, r.rolcreatedb, r.rolcreaterole, r.rolreplication,
               r.rolbypassrls, r.rolinherit, d.datdba=r.oid AS owns_database,
               EXISTS (SELECT 1 FROM pg_auth_members WHERE member=r.oid) AS memberships
        FROM pg_database d JOIN pg_roles r ON r.rolname=current_user
        WHERE d.datname=current_database()
    """))).mappings().one()
    if any(row[k] != identity[k] for k in ('database', 'role', 'oid')):
        raise CopyRefused('Database identity differs')
    if any(row[k] for k in ('rolsuper', 'rolcreatedb', 'rolcreaterole', 'rolreplication', 'rolbypassrls', 'rolinherit', 'memberships')):
        raise CopyRefused('Role authority refused')
    if row['owns_database'] != owner:
        raise CopyRefused('Database ownership refused')
    await require_unfiltered_reads(connection)
    if owner:
        count = (await connection.execute(text("""
            SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname='public' AND c.relkind IN ('r','p','S')
              AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e')
              AND c.relowner <> (SELECT oid FROM pg_roles WHERE rolname=current_user)
        """))).scalar_one()
        if count:
            raise CopyRefused('Target objects are not owned by migrator')
        sessions = (await connection.execute(text('SELECT count(*) FROM pg_stat_activity WHERE datid=:oid AND pid<>pg_backend_pid()'), {'oid': identity['oid']})).scalar_one()
        if sessions:
            raise CopyRefused('Other target sessions exist')
    else:
        writes = (await connection.execute(text("""
            SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE n.nspname='public' AND c.relkind IN ('r','p') AND
              (has_table_privilege(c.oid,'INSERT,UPDATE,DELETE,TRUNCATE,TRIGGER')
               OR has_any_column_privilege(c.oid,'INSERT,UPDATE'))
        """))).scalar_one()
        if writes:
            raise CopyRefused('Source role is not read-only')


def import_order(catalog):
    dependencies = {}
    for name, table in catalog['tables'].items():
        dependencies[name] = set()
        for constraint in table['constraints']:
            if constraint['kind'] != 'f' or (name, constraint['name']) in CYCLIC_FKS:
                continue
            match = re.search(r'REFERENCES (?:public\.)?"?([a-z_]+)"?\(', constraint['definition'])
            if not match:
                raise CopyRefused('Unreviewed foreign key syntax')
            dependencies[name].add(match.group(1))
    result = []
    while dependencies:
        ready = sorted(name for name, parents in dependencies.items() if not parents.intersection(dependencies))
        if not ready:
            raise CopyRefused('Unreviewed foreign key cycle')
        result.extend(ready)
        for name in ready:
            del dependencies[name]
    return result


async def change_cyclic_fks(target, catalog, *, restore):
    for name, constraint_name in sorted(CYCLIC_FKS):
        constraint = next(c for c in catalog['tables'][name]['constraints'] if c['name'] == constraint_name)
        if constraint['kind'] != 'f' or not constraint['validated']:
            raise CopyRefused('Unreviewed cyclic constraint')
        await target.execute(text(f'ALTER TABLE {relation(name)} DROP CONSTRAINT {quote(constraint_name)}'))
        suffix = '' if restore else ' DEFERRABLE INITIALLY DEFERRED'
        await target.execute(text(f'ALTER TABLE {relation(name)} ADD CONSTRAINT {quote(constraint_name)} {constraint["definition"]}{suffix}'))


async def named_triggers(target, catalog, *, enable):
    for name, table in catalog['tables'].items():
        for trigger in table['triggers']:
            if trigger['enabled'] != 'O':
                raise CopyRefused('Unexpected trigger mode')
            operation = 'ENABLE' if enable else 'DISABLE'
            await target.execute(text(f'ALTER TABLE {relation(name)} {operation} TRIGGER {quote(trigger["name"])}'))


async def pristine_target(target, manifest, catalog):
    execution = manifest['execution']
    if catalog['reference_rows'] != execution['baseline_reference_rows']:
        raise CopyRefused('Target reference baseline differs')
    if await sequence_states(target, catalog) != execution['baseline_sequences']:
        raise CopyRefused('Target sequence baseline differs; rebuild required')
    for name in catalog['tables']:
        if name in ('products', 'delivery_points', 'alembic_version'):
            continue
        if (await target.execute(text(f'SELECT EXISTS (SELECT 1 FROM {relation(name)})'))).scalar_one():
            raise CopyRefused('Target has business rows; rebuild required')


async def copy_table(source, target, manifest, name):
    mapping = manifest['tables'][name]
    additions = {key: None if value['value'] is None else str(value['value']) for key, value in mapping['target_additions'].items()}
    table_map = TableMap(name, tuple(mapping['columns']), additions)
    names = (*table_map.copied_columns, *additions)
    statement = insert_statement(manifest['target_catalog']['tables'][name], names)
    baseline = {row['id']: row for row in manifest['execution']['baseline_reference_rows'].get(name, [])}
    async for batch in text_batches(source, manifest['source_catalog']['tables'][name], table_map.copied_columns):
        values = []
        for row in batch:
            mapped = map_row(row, table_map)
            if mapping['classification'] == 'regenerate' and row['id'] in baseline:
                existing = baseline[row['id']]
                # Identity fields must agree. Mutable catalog state and timestamps
                # are source evidence and replace migration-generated values.
                identity_fields = ('name', 'fuel_type', 'fuel_grade', 'unit') if name == 'products' else ('name',)
                if any(existing[key] != row[key] for key in identity_fields):
                    raise CopyRefused(f'Reference identity conflict: {name}')
                await target.execute(text(f'DELETE FROM {relation(name)} WHERE id=CAST(:id AS uuid)'), {'id': row['id']})
            values.append({f'v{index}': mapped[column] for index, column in enumerate(names)})
        if values:
            await target.execute(statement, values)


async def set_sequences(source, target, manifest):
    source_states = await sequence_states(source, manifest['source_catalog'])
    results = {}
    if manifest['source_catalog']['sequences'] != manifest['target_catalog']['sequences']:
        raise CopyRefused('Sequence definitions differ')
    for definition in manifest['target_catalog']['sequences']:
        name = definition['name']
        if definition['owner_table']:
            table, column = definition['owner_table'], definition['owner_column']
        elif name in GLOBAL_SEQUENCES:
            table, column = GLOBAL_SEQUENCES[name]
        else:
            raise CopyRefused('Unclassified global sequence')
        maximum = (await target.execute(text(f'SELECT max({quote(column)}) FROM {relation(table)}'))).scalar_one()
        state = source_states[name]
        value, called, next_value = next_sequence_state(definition, state['last_value'], state['is_called'], maximum)
        await target.execute(text('SELECT setval(CAST(:name AS regclass), :value, :called)'), {'name': relation(name), 'value': value, 'called': called})
        results[name] = dict(source=state, last_value=value, is_called=called, next_value=next_value)
    actual = await sequence_states(target, manifest['target_catalog'])
    if any(actual[name] != dict(last_value=row['last_value'], is_called=row['is_called']) for name, row in results.items()):
        raise CopyRefused('Target sequence verification failed')
    return results


async def probe_trade_guard(target):
    count = (await target.execute(text('SELECT count(*) FROM trades'))).scalar_one()
    if not count:
        raise CopyRefused('Guard proof requires reviewed trade probe fixture')
    savepoint = await target.begin_nested()
    try:
        await target.execute(text("""
            INSERT INTO trades (id,buyer_id,seller_id,buyer_provenance,seller_provenance,
                                initiated_by,quantity_mt,price_per_mt_usd,status,market_snapshot_version)
            SELECT gen_random_uuid(),t.buyer_id,t.seller_id,b.provenance,s.provenance,
                   t.initiated_by,t.quantity_mt,t.price_per_mt_usd,t.status,NULL
            FROM trades t JOIN organizations b ON b.id=t.buyer_id
                          JOIN organizations s ON s.id=t.seller_id
            ORDER BY t.id LIMIT 1
        """))
    except Exception as error:
        # This exact trigger signal, not an unrelated NOT NULL/FK failure, is proof.
        if 'market snapshot version 1' not in str(error).lower():
            raise ValueError(f'Restored trade guard probe failed: {error}') from None
    else:
        raise CopyRefused('Restored trade guard did not reject probe')
    finally:
        await savepoint.rollback()


async def copy_database(manifest, source, target, *, fail_at=None) -> CopyReceipt:
    stage = 'preflight'
    receipt_path = None
    started = False
    def boundary(name):
        nonlocal stage
        stage = name
        if fail_at == name:
            raise CopyRefused(f'Injected failure: {name}')
    try:
        validate_manifest(manifest)
        if source.in_transaction() or target.in_transaction():
            raise CopyRefused('Copy owns both transactions')
        execution = manifest['execution']
        receipt_path = Path(execution['receipt_path'])
        attempt_path = receipt_path.with_suffix('.attempt.json')
        if os.path.lexists(receipt_path) or os.path.lexists(attempt_path):
            raise CopyRefused('Prior import attempt exists; verify or rebuild, never replay')
        async with source.begin():
            await source.execute(text('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY'))
            await configure_text_session(source)
            await source.execute(text("SET LOCAL search_path=public,pg_catalog"))
            await attest(source, execution['source'], owner=False)
            source_catalog = await source.run_sync(inspect_catalog)
            require_complete_inventory(source_catalog, manifest['source_catalog'])
            await check_historical_invariants(source)
            await notice_receipts(source)
            snapshot = (await source.execute(text('SELECT pg_current_snapshot()::text'))).scalar_one()
            async with target.begin():
                await configure_text_session(target)
                await target.execute(text("SET LOCAL search_path=public,pg_catalog"))
                await target.execute(text("SET LOCAL lock_timeout='3s'"))
                locked = (await target.execute(text('SELECT pg_try_advisory_xact_lock(:key)'), {'key': IMPORT_LOCK})).scalar_one()
                if not locked:
                    raise CopyRefused('Import lock is busy')
                await attest(target, execution['target'], owner=True)
                target_catalog = await target.run_sync(inspect_catalog)
                require_complete_inventory(target_catalog, manifest['target_catalog'])
                for name in sorted(target_catalog['tables']):
                    await target.execute(text(f'LOCK TABLE {relation(name)} IN ACCESS EXCLUSIVE MODE'))
                await pristine_target(target, manifest, target_catalog)
                private_json(attempt_path, dict(manifest_digest=digest(manifest), target_oid=execution['target']['oid']))
                started = True
                boundary('before_data')
                await change_cyclic_fks(target, target_catalog, restore=False)
                await named_triggers(target, target_catalog, enable=False)
                for name in import_order(target_catalog):
                    mapping = manifest['tables'].get(name)
                    if mapping and mapping['classification'] != 'skip':
                        await copy_table(source, target, manifest, name)
                        if name == 'rfq_quotes':
                            boundary('during_cycle')
                for identity, sent_at in (await notice_receipts(source)).items():
                    await target.execute(text('INSERT INTO kyc_grace_notices (grace_id,claimed_at,sent_at) VALUES (:id,:time,:time)'), {'id': identity, 'time': sent_at})
                await target.execute(text('SET CONSTRAINTS ALL IMMEDIATE'))
                await change_cyclic_fks(target, target_catalog, restore=True)
                await named_triggers(target, target_catalog, enable=True)
                require_complete_inventory(await target.run_sync(inspect_catalog), target_catalog)
                await probe_trade_guard(target)
                boundary('after_trigger_restoration')
                sequences = await set_sequences(source, target, manifest)
                boundary('after_sequences')
                verification = await verify_database(manifest, source, target)
                receipt = CopyReceipt(digest(manifest), snapshot, execution['target']['oid'], verification, sequences)
            boundary('after_commit')
        boundary('before_receipt')
        private_json(receipt_path, asdict(receipt))
        return receipt
    except Exception as error:
        if started and receipt_path is not None:
            try:
                private_json(receipt_path.with_suffix('.failure.json'), dict(stage=stage, error=str(error)))
            except OSError:
                pass
        raise CopyRefused(f'Copy refused at {stage}; inspect private diagnostic report') from None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--source-environment-file', required=True)
    parser.add_argument('--target-environment-file', required=True)
    parser.add_argument('--source-sha', required=True)
    parser.add_argument('--target-sha', required=True)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    try:
        manifest_text = read_private_file(args.manifest, root_owned=os.geteuid() == 0)
        manifest = json.loads(manifest_text)
        validate_manifest(manifest)
        if args.source_sha != manifest['source_sha'] or args.target_sha != manifest['target_base_sha']:
            raise CopyRefused('Release SHA differs')
        if not args.apply:
            print('Manifest valid; no connections opened; no mutation authorized')
            return
        urls = []
        for path in (args.source_environment_file, args.target_environment_file):
            content = read_private_file(path)
            lines = content.splitlines()
            if len(lines) != 1 or not lines[0].startswith('DATABASE_URL='):
                raise CopyRefused('Expected one explicit DATABASE_URL entry')
            urls.append(validate_url(lines[0].split('=', 1)[1], manifest['execution']))
        async def run():
            engines = [create_async_engine(url, hide_parameters=True) for url in urls]
            try:
                async with engines[0].connect() as source, engines[1].connect() as target:
                    await copy_database(manifest, source, target)
            finally:
                for engine in engines:
                    await engine.dispose()
        asyncio.run(run())
        print('Copy committed and private receipt published')
    except Exception:
        parser.exit(1, 'Copy refused; inspect private host evidence\n')


if __name__ == '__main__':
    main()
