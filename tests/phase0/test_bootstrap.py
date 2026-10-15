"""The checked connection, not Settings or Config URLs, owns bootstrap DDL."""
from __future__ import annotations

import os
from dataclasses import replace
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
REVISION = 'kycd_20261015_staging'


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
                           cluster_system_identifier=os.environ['PHASE0_CLUSTER_ID'])
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
    assert before == [await fingerprint(name) for name in ('phase0_staging_test', 'phase0_prod_test')]


@pytest.mark.asyncio
@pytest.mark.parametrize('field,value', [('oid', 1), ('host', '127.0.0.2'), ('port', 1),
    ('database', 'phase0_prod_test'), ('role', 'postgres'), ('server_host', '127.0.0.2'),
    ('server_port', 1), ('cluster_system_identifier', '1')])
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
async def test_nonempty_target_refused(target):
    from scripts.phase0.bootstrap import BootstrapIdentity, bootstrap_connection
    engine, values = target
    async with engine.begin() as connection:
        await connection.execute(text('CREATE TABLE unexpected (id integer)'))
    async with engine.connect() as connection:
        with pytest.raises(ValueError, match='empty'):
            await bootstrap_connection(connection, BootstrapIdentity(**values), config(), apply=True)
