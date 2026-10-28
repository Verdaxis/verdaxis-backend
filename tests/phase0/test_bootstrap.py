"""The checked connection, not Settings or Config URLs, owns bootstrap DDL."""
from __future__ import annotations

import json
import os
import subprocess
import sys
from dataclasses import asdict, replace
from pathlib import Path
from uuid import uuid4

import pytest
import pytest_asyncio
from alembic.config import Config
from sqlalchemy import event, text
from sqlalchemy.ext.asyncio import create_async_engine

from alembic import command
from scripts.phase0.manifest import digest, inspect_catalog

ROOT = Path(__file__).resolve().parents[2]
REVISION = 'p0_20261016_union'


def url(database, role='phase0_owner'):
    port = int(os.environ['PHASE0_PORT'])
    assert 1024 < port < 65536
    return f"postgresql+asyncpg://{role}:{os.environ['PHASE0_PASSWORD']}@127.0.0.1:{port}/{database}"


@pytest_asyncio.fixture
async def target():
    name = 'phase0_bootstrap_' + uuid4().hex
    admin = create_async_engine(url('postgres', 'postgres'), isolation_level='AUTOCOMMIT')
    async with admin.connect() as connection:
        await connection.execute(text(f'CREATE DATABASE "{name}" OWNER phase0_owner'))
        server = (await connection.execute(text('SELECT inet_server_addr()::text, inet_server_port()'))).one()
    engine = create_async_engine(url(name))
    try:
        async with engine.connect() as connection:
            oid = (await connection.execute(text('SELECT oid FROM pg_database WHERE datname=current_database()'))).scalar_one()
        yield engine, dict(database=name, oid=oid, role='phase0_owner', host='127.0.0.1',
                           port=int(os.environ['PHASE0_PORT']), server_host=server[0], server_port=server[1],
                           cluster_system_identifier=os.environ['PHASE0_CLUSTER_ID'],
                           source_sha=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip())
    finally:
        await engine.dispose()
        async with admin.connect() as connection:
            await connection.execute(text(f'DROP DATABASE "{name}"'))
        await admin.dispose()


def config():
    result = Config(str(ROOT / 'alembic.ini'))
    result.set_main_option('script_location', str(ROOT / 'alembic'))
    result.set_main_option('sqlalchemy.url', url('phase0_prod_test'))
    return result


async def fingerprint(database):
    engine = create_async_engine(url(database))
    try:
        async with engine.connect() as connection:
            catalog = await connection.run_sync(inspect_catalog)
            rows = {}
            for table in catalog['tables']:
                rows[table] = list((await connection.execute(text(
                    f'SELECT row_to_json(t)::text FROM "{table}" t ORDER BY row_to_json(t)::text'
                ))).scalars())
            return digest(dict(catalog=catalog, rows=rows))
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_supplied_connection_is_not_replaced(target, monkeypatch):
    # This reaches the old env.py before the bootstrap helper exists.
    from app.config import settings
    monkeypatch.setattr(settings, 'DATABASE_URL', url('phase0_staging_test'))
    monkeypatch.setattr(settings, 'MIGRATOR_DATABASE_URL', url('phase0_staging_test'))
    engine, _ = target
    cfg = config()
    async with engine.connect() as connection, connection.begin():
        def upgrade(sync):
            cfg.attributes['connection'] = sync
            with pytest.raises(ValueError, match='bootstrap identity'):
                command.upgrade(cfg, REVISION)
        await connection.run_sync(upgrade)


@pytest.mark.asyncio
async def test_bootstrap_uses_only_checked_connection(target, monkeypatch):
    from app.config import settings
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    monkeypatch.setattr(settings, 'DATABASE_URL', url('phase0_staging_test'))
    monkeypatch.setattr(settings, 'MIGRATOR_DATABASE_URL', url('phase0_staging_test'))
    before = [await fingerprint(name) for name in ('phase0_staging_test', 'phase0_prod_test')]
    engine, values = target
    identity = BootstrapIdentity(**values)
    cfg = config()
    await engine.dispose()
    connects = []
    event.listen(engine.sync_engine, 'connect', lambda *_: connects.append(1))
    async with engine.connect() as connection:
        await bootstrap_connection(connection, identity, cfg, apply=True)
        assert not connection.in_transaction()
        assert (await connection.execute(text('SELECT version_num FROM alembic_version'))).scalar_one() == REVISION
        for table in ('users', 'organizations', 'kyc_execution_grace', 'kyc_grace_notices', 'kyc_documents'):
            assert (await connection.execute(text(f'SELECT count(*) FROM {table}'))).scalar_one() == 0
        assert not (await connection.execute(text("SELECT rolsuper OR rolcreatedb OR rolcreaterole OR rolreplication OR rolbypassrls FROM pg_roles WHERE rolname=current_user"))).scalar_one()
    assert connects == [1]
    assert cfg.attributes == {}
    assert cfg.get_main_option('sqlalchemy.url') == url('phase0_prod_test')
    after = [await fingerprint(name) for name in ('phase0_staging_test', 'phase0_prod_test')]
    assert before == after
    evidence = dict(identity=asdict(identity), engine_connections=len(connects),
                    unrelated_hashes_before=before, unrelated_hashes_after=after,
                    transaction='committed after schema verification')
    (Path(os.environ['PHASE0_WORK']) / 'bootstrap-evidence.json').write_text(
        json.dumps(evidence, indent=2) + '\n'
    )


@pytest.mark.asyncio
@pytest.mark.parametrize('field,value', [('oid', 1), ('host', '127.0.0.2'), ('port', 1),
    ('database', 'phase0_prod_test'), ('role', 'postgres'), ('server_host', '127.0.0.2'),
    ('server_port', 1), ('cluster_system_identifier', '1'), ('source_sha', '0' * 40)])
async def test_wrong_identity_refused_before_ddl(target, field, value):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    identity = replace(BootstrapIdentity(**values), **{field: value})
    async with engine.connect() as connection:
        with pytest.raises(ValueError):
            await bootstrap_connection(connection, identity, config(), apply=True)
        assert (await connection.execute(text("SELECT to_regclass('alembic_version')"))).scalar_one() is None


@pytest.mark.asyncio
async def test_approval_and_outer_transaction_required(target):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    identity = BootstrapIdentity(**values)
    async with engine.connect() as connection:
        with pytest.raises(ValueError, match='approval'):
            await bootstrap_connection(connection, identity, config(), apply=False)
        def upgrade(sync):
            cfg = config()
            cfg.attributes.update(connection=sync, phase0_bootstrap_identity=identity)
            with pytest.raises(ValueError, match='outer transaction'):
                command.upgrade(cfg, REVISION)
        await connection.run_sync(upgrade)


@pytest.mark.asyncio
@pytest.mark.parametrize('failure', ['verification', 'autocommit'])
async def test_failure_rolls_back_all_ddl(target, monkeypatch, failure):
    from alembic.runtime.migration import MigrationContext

    from scripts.phase0 import bootstrap
    engine, values = target
    cfg = config()
    if failure == 'verification':
        def fail(*_):
            raise ValueError('injected verification failure')
        monkeypatch.setattr(bootstrap, 'verify_bootstrap_schema', fail)
    else:
        original = MigrationContext.run_migrations
        def escape(self, **kw):
            original(self, **kw)
            with self.autocommit_block():
                pass
        monkeypatch.setattr(MigrationContext, 'run_migrations', escape)
    async with engine.connect() as connection:
        with pytest.raises(ValueError, match='verification|autocommit'):
            await bootstrap.bootstrap_connection(connection, bootstrap.BootstrapIdentity(**values), cfg, apply=True)
        assert cfg.attributes == {}
        assert (await connection.execute(text("SELECT to_regclass('users')"))).scalar_one() is None
        assert (await connection.execute(text("SELECT to_regclass('alembic_version')"))).scalar_one() is None


@pytest.mark.asyncio
@pytest.mark.parametrize('ddl', [
    'CREATE TABLE unexpected (id integer)',
    'CREATE SEQUENCE unexpected',
    'CREATE VIEW unexpected AS SELECT 1 AS id',
])
async def test_nonempty_target_refused(target, ddl):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    async with engine.begin() as connection:
        await connection.execute(text(ddl))
    async with engine.connect() as connection:
        with pytest.raises(ValueError, match='empty'):
            await bootstrap_connection(connection, BootstrapIdentity(**values), config(), apply=True)


@pytest.mark.asyncio
@pytest.mark.parametrize('revision,approved,offline,match', [
    (REVISION, False, False, 'approval'),
    ('head', True, False, 'literal'),
    (REVISION, True, True, 'Invalid bootstrap connection'),
])
async def test_direct_supplied_path_rejects_missing_approval_and_aliases(
    target, revision, approved, offline, match
):
    from scripts.phase0.bootstrap import BootstrapIdentity
    engine, values = target
    async with engine.connect() as connection, connection.begin():
        def upgrade(sync):
            cfg = config()
            cfg.attributes.update(connection=sync,
                phase0_bootstrap_identity=BootstrapIdentity(**values),
                phase0_bootstrap_approved=approved)
            with pytest.raises(ValueError, match=match):
                command.upgrade(cfg, revision, sql=offline)
        await connection.run_sync(upgrade)
        assert (await connection.execute(text("SELECT to_regclass('users')"))).scalar_one() is None


@pytest.mark.asyncio
async def test_bootstrap_lock_blocks_second_connection(target):
    from scripts.phase0.bootstrap import (
        BOOTSTRAP_LOCK,
        BootstrapIdentity,
        bootstrap_connection,
    )
    engine, values = target
    async with engine.begin() as first:
        await first.execute(text('SELECT pg_advisory_xact_lock(:key)'), {'key': BOOTSTRAP_LOCK})
        async with engine.connect() as second:
            with pytest.raises(ValueError, match='lock is busy'):
                await bootstrap_connection(second, BootstrapIdentity(**values), config(), apply=True)


@pytest.mark.asyncio
async def test_connected_cluster_checked_against_independent_binding(target, monkeypatch):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    monkeypatch.setenv('PHASE0_CLUSTER_ID', '1')
    values['cluster_system_identifier'] = '1'
    async with engine.connect() as connection:
        with pytest.raises(ValueError, match='connected cluster'):
            await bootstrap_connection(connection, BootstrapIdentity(**values), config(), apply=True)


@pytest.mark.asyncio
@pytest.mark.parametrize('apply', [False, True])
async def test_cli_requires_apply_and_private_target_file(target, tmp_path, apply):
    from scripts.phase0.bootstrap import BootstrapIdentity
    engine, values = target
    manifest = tmp_path / 'manifest.json'
    manifest.write_text(json.dumps(dict(mode='synthetic', approved=True,
        target_revision=REVISION, identity=asdict(BootstrapIdentity(**values)))))
    manifest.chmod(0o600)
    environment_file = tmp_path / 'target.env'
    if apply:
        environment_file.write_text('MIGRATOR_DATABASE_URL=' + url(values['database']))
        environment_file.chmod(0o600)
    result = subprocess.run([sys.executable, '-m', 'scripts.phase0.bootstrap',
        '--manifest', str(manifest), '--target-environment-file', str(environment_file),
        *(['--apply'] if apply else [])], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert ('Bootstrap applied' if apply else 'no connection opened') in result.stdout
    async with engine.connect() as connection:
        exists = (await connection.execute(text("SELECT to_regclass('users')"))).scalar_one()
        assert (exists is not None) == apply


@pytest.mark.asyncio
@pytest.mark.parametrize('attribute,reset', [('INHERIT', 'NOINHERIT'), ('CREATEDB', 'NOCREATEDB')])
async def test_role_properties_checked_on_ddl_connection(target, attribute, reset):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    admin = create_async_engine(url('postgres', 'postgres'), isolation_level='AUTOCOMMIT')
    try:
        async with admin.connect() as connection:
            await connection.execute(text(f'ALTER ROLE phase0_owner {attribute}'))
        async with engine.connect() as connection:
            with pytest.raises(ValueError, match='role authority'):
                await bootstrap_connection(connection, BootstrapIdentity(**values), config(), apply=True)
    finally:
        async with admin.connect() as connection:
            await connection.execute(text(f'ALTER ROLE phase0_owner {reset}'))
        await admin.dispose()


@pytest.mark.asyncio
@pytest.mark.parametrize('isolation_source', ['engine', 'connection'])
@pytest.mark.parametrize('entry', ['helper', 'direct'])
async def test_autocommit_refused_before_ddl(target, isolation_source, entry):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection

    _, values = target
    options = {'isolation_level': 'AUTOCOMMIT'} if isolation_source == 'engine' else {}
    engine = create_async_engine(url(values['database']), **options)
    ddl = []

    def record_ddl(connection, cursor, statement, parameters, context, executemany):
        if statement.lstrip().split()[0].upper() in ('CREATE', 'ALTER', 'DROP'):
            ddl.append(statement)

    event.listen(engine.sync_engine, 'before_cursor_execute', record_ddl)
    try:
        async with engine.connect() as connection:
            if isolation_source == 'connection':
                await connection.execution_options(isolation_level='AUTOCOMMIT')
            identity = BootstrapIdentity(**values)
            cfg = config()
            with pytest.raises(ValueError, match='physical transaction'):
                if entry == 'helper':
                    await bootstrap_connection(connection, identity, cfg, apply=True)
                else:
                    async with connection.begin():
                        def upgrade(sync):
                            cfg.attributes.update(connection=sync,
                                phase0_bootstrap_identity=identity,
                                phase0_bootstrap_approved=True)
                            command.upgrade(cfg, REVISION)
                        await connection.run_sync(upgrade)
            assert ddl == []
            assert (await connection.execute(text(
                "SELECT to_regclass('users'), to_regclass('alembic_version')"
            ))).one() == (None, None)
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_mid_migration_failure_rolls_back_schema(target):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection

    engine, values = target
    observed = []

    def fail_after_ddl(connection, cursor, statement, parameters, context, executemany):
        if statement.lstrip().startswith('CREATE TABLE organizations'):
            observed.append(connection.execute(text("SELECT to_regclass('organizations')")).scalar_one())
            raise ValueError('injected mid-migration failure')

    event.listen(engine.sync_engine, 'after_cursor_execute', fail_after_ddl)
    cfg = config()
    try:
        async with engine.connect() as connection:
            with pytest.raises(ValueError, match='mid-migration'):
                await bootstrap_connection(connection, BootstrapIdentity(**values), cfg, apply=True)
            assert observed == ['organizations']
            assert cfg.attributes == {}
            assert (await connection.execute(text(
                "SELECT to_regclass('organizations'), to_regclass('alembic_version')"
            ))).one() == (None, None)
    finally:
        event.remove(engine.sync_engine, 'after_cursor_execute', fail_after_ddl)


@pytest.mark.asyncio
@pytest.mark.parametrize('outcome', ['commit', 'rollback'])
async def test_bootstrap_lock_held_through_verification(target, monkeypatch, outcome):
    from scripts.phase0 import bootstrap

    engine, values = target
    original_verify = bootstrap.verify_bootstrap_schema
    checks = []
    async with engine.connect() as observer:
        def verify(connection, policy):
            original_verify(connection, policy)
            # run_sync permits a second asyncpg connection through its sync adapter.
            acquired = observer.sync_connection.execute(text(
                'SELECT pg_try_advisory_xact_lock(:key)'
            ), {'key': bootstrap.BOOTSTRAP_LOCK}).scalar_one()
            checks.append(acquired)
            assert acquired is False
            observer.sync_connection.rollback()
            if outcome == 'rollback':
                raise ValueError('injected locked verification failure')

        monkeypatch.setattr(bootstrap, 'verify_bootstrap_schema', verify)
        async with engine.connect() as connection:
            if outcome == 'rollback':
                with pytest.raises(ValueError, match='locked verification'):
                    await bootstrap.bootstrap_connection(connection,
                        bootstrap.BootstrapIdentity(**values), config(), apply=True)
            else:
                await bootstrap.bootstrap_connection(connection,
                    bootstrap.BootstrapIdentity(**values), config(), apply=True)
            assert not connection.in_transaction()
        assert checks == [False]
        assert (await observer.execute(text('SELECT pg_try_advisory_xact_lock(:key)'),
            {'key': bootstrap.BOOTSTRAP_LOCK})).scalar_one() is True
        await observer.rollback()


@pytest.mark.asyncio
async def test_ordinary_path_still_uses_settings_url(target):
    engine, values = target
    environment = dict(os.environ, DATABASE_URL=url(values['database']),
                       MIGRATOR_DATABASE_URL=url(values['database']))
    result = subprocess.run([sys.executable, '-m', 'alembic', 'upgrade', REVISION],
        env=environment, capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    async with engine.connect() as connection:
        assert (await connection.execute(text('SELECT version_num FROM alembic_version'))).scalar_one() == REVISION
