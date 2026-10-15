"""Exercise the operator assertions against the runner-owned cluster only."""
import asyncio
import os
import socket

import asyncpg
import pytest
import pytest_asyncio

from scripts.phase0 import assert_host


@pytest_asyncio.fixture
async def assertion_database(monkeypatch):
    port = int(os.environ["PHASE0_PORT"])
    password = os.environ["PHASE0_PASSWORD"]
    admin = await asyncpg.connect(host="127.0.0.1", port=port, user="postgres",
                                  password=password, database="postgres")
    await admin.execute("CREATE ROLE verdaxis_backup LOGIN NOINHERIT")
    await admin.execute("CREATE ROLE verdaxis_app LOGIN NOINHERIT")
    # The runner generates a hex-only disposable password, never a live secret.
    assert all(character in "0123456789abcdef" for character in password)
    await admin.execute(f"ALTER ROLE verdaxis_backup PASSWORD '{password}'")
    await admin.execute("CREATE DATABASE verdaxis")
    connection = await asyncpg.connect(host="127.0.0.1", port=port, user="postgres",
                                       password=password, database="verdaxis")
    await connection.execute("""
        CREATE TABLE alembic_version (version_num text);
        INSERT INTO alembic_version VALUES ('kycg_20261014_prod');
        CREATE TABLE market_event_outbox (stream_seq bigint);
        GRANT SELECT ON alembic_version, market_event_outbox TO verdaxis_backup;
    """)
    oid = await connection.fetchval("SELECT oid FROM pg_database WHERE datname=current_database()")
    server_host, server_port = await connection.fetchrow(
        'SELECT host(inet_server_addr()), inet_server_port()')
    await connection.close()
    # Colima NAT changes the server-visible endpoint. Check it exactly, then
    # translate only that known mapping; production retains strict loopback checks.
    require_identity = assert_host.require_identity

    def require_container_identity(expected, actual):
        if actual is not expected:
            assert (actual['host'], actual['port']) == (server_host, server_port)
            actual = dict(actual, host='127.0.0.1', port=port)
        require_identity(expected, actual)

    monkeypatch.setattr(assert_host, 'require_identity', require_container_identity)
    manifest = dict(hostname=socket.gethostname().split('.')[0], password_file='/synthetic',
                    identity=dict(database='verdaxis', database_oid=oid,
                                  cluster_id=os.environ['PHASE0_CLUSTER_ID'],
                                  host='127.0.0.1', port=port, role='verdaxis_backup',
                                  revision='kycg_20261014_prod'))
    monkeypatch.setattr(assert_host, 'private_bytes', lambda path: password.encode())
    monkeypatch.setattr(assert_host, 'command', lambda args: os.environ['PHASE0_CLUSTER_ID'])
    monkeypatch.setattr(assert_host, 'stopped_units', lambda: None)
    try:
        yield manifest, dict(host='127.0.0.1', port=port, user='postgres',
                            password=password, database='verdaxis')
    finally:
        await admin.execute('DROP DATABASE verdaxis')
        await admin.execute('DROP ROLE verdaxis_backup')
        await admin.execute('DROP ROLE verdaxis_app')
        await admin.close()


@pytest.mark.asyncio
async def test_declared_driver_runs_identity_and_post_stop_assertions(assertion_database):
    manifest, _ = assertion_database
    await asyncio.to_thread(assert_host.run_check, manifest, 'identity')
    await asyncio.to_thread(assert_host.run_check, manifest, 'outbox-empty')


@pytest.mark.asyncio
@pytest.mark.parametrize('role', ['postgres', 'verdaxis_backup'])
async def test_post_stop_gate_refuses_every_other_session(assertion_database, role):
    manifest, arguments = assertion_database
    arguments['user'] = role
    other = await asyncpg.connect(**arguments)
    try:
        # Even an idle backup/psql connection must block the snapshot boundary.
        await other.execute('BEGIN')
        with pytest.raises(ValueError, match='post-stop outbox'):
            await asyncio.to_thread(assert_host.run_check, manifest, 'outbox-empty')
        await other.execute('ROLLBACK')
    finally:
        await other.close()
    await asyncio.to_thread(assert_host.run_check, manifest, 'outbox-empty')
