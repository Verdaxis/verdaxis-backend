"""Root-container-only inputs for the real production CLI acceptance test."""
import asyncio
import hashlib
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from sqlalchemy.ext.asyncio import create_async_engine
from sqlalchemy.pool import NullPool

from scripts.phase0.host_authority import tree_digest
from scripts.phase0.manifest import digest, load_manifest
from scripts.phase0.verify import fingerprint_database

ROOT = Path('/app')


@dataclass(frozen=True)
class ToolResult:
    """Result of one real CLI subprocess, shaped like subprocess.CompletedProcess."""

    returncode: int
    stdout: str
    stderr: str


def private_file(path, content):
    path.write_text(content)
    path.chmod(0o600)


async def settle_quiesced(scratch, *, attempts=40):
    """Wait until the disposable source and target have no other sessions.

    The fixture's own helper connections close asynchronously; the tools
    require a genuinely quiesced source and an isolated target.
    """
    for _ in range(attempts):
        async with scratch.admin_connection() as admin:
            others = await admin.fetchval('''
                SELECT count(*) FROM pg_stat_activity
                WHERE datname IN ('verdaxis', 'verdaxis_prod') AND pid <> pg_backend_pid()
            ''')
        if others == 0:
            return
        await asyncio.sleep(0.25)
    raise ValueError('Disposable databases did not settle to quiescence')


def install_authority(common):
    """Write the operator authority from the current claims.

    Observed-state tests call this after changing identity claims, so manifest
    and authority agree and only the disposable observed state differs.
    """
    authority_directory = Path('/root/verdaxis-phase0')
    authority_directory.mkdir(mode=0o700, exist_ok=True)
    authority = {key: value for key, value in common.items() if key != 'bootstrap_receipt_sha256'}
    private_file(authority_directory / 'authority.json', json.dumps(authority))


async def make_case(scratch, directory):
    if os.geteuid() != 0:
        raise ValueError('Real container-root authority required')
    directory.mkdir(mode=0o700)
    engine = create_async_engine(scratch.source_url, poolclass=NullPool)
    try:
        async with engine.connect() as source:
            frozen = await fingerprint_database(source, load_manifest()['source_catalog'])
    finally:
        await engine.dispose()
    async with scratch.admin_connection() as admin:
        acl = await admin.fetchval("SELECT datacl::text FROM pg_database WHERE datname='verdaxis_prod'")
    release = json.loads(Path('/release.json').read_text())
    common = dict(mode='production', approved=True, hostname=os.uname().nodename,
                  release_sha=release['release_sha'], tool_tree_sha256=tree_digest(ROOT),
                  source_sha=load_manifest()['source_sha'],
                  source= scratch.source, target=scratch.target,
                  source_catalog_sha256=digest(load_manifest()['source_catalog']),
                  frozen_source=frozen, target_datacl=acl,
                  bootstrap_receipt=str(directory / 'bootstrap-report.json'),
                  bootstrap_receipt_sha256=None)
    install_authority(common)
    environment = dict(os.environ, ENVIRONMENT='production', RELEASE_SHA=release['release_sha'],
                       DATABASE_NAME='verdaxis_prod', DATABASE_USER='verdaxis_app',
                       DATABASE_HOST='127.0.0.1', DATABASE_PORT=str(scratch.port),
                       DATABASE_PASSWORD=scratch.passwords['verdaxis_app'], DATABASE_URL=scratch.app_url,
                       MIGRATOR_DATABASE_URL=scratch.target_url,
                       JWT_SECRET='isolated-production-synthetic-secret-123456789',
                       KYC_EXECUTION_GATE_ENABLED='false')
    for tool in ('bootstrap', 'copy', 'verify'):
        for side, url in (('source', scratch.source_url), ('target', scratch.target_url)):
            private_file(directory / f'{tool}-{side}.env', 'DATABASE_URL=' + url + '\n')
    await settle_quiesced(scratch)
    return common, environment


def tool_command(tool, directory, common, *, suffix='', manifest_tool=None, apply=True):
    """Write one per-tool manifest and return its documented production command."""
    manifest = dict(common, tool=manifest_tool or tool, report=str(directory / f'{tool}{suffix}-report.json'))
    path = directory / f'{tool}{suffix}.json'
    private_file(path, json.dumps(manifest))
    command = [sys.executable, '-m', f'scripts.phase0.{tool}', '--manifest', str(path),
               '--source-environment-file', str(directory / f'{tool}-source.env'),
               '--target-environment-file', str(directory / f'{tool}-target.env')]
    if tool == 'copy':
        command += ['--source-sha', common['source_sha'], '--target-sha', common['release_sha']]
    if tool == 'verify':
        command += ['--report', manifest['report']]
    elif apply:
        command += ['--apply']
    return command


async def invoke(tool, directory, common, environment, *, suffix='', manifest_tool=None, apply=True):
    command = tool_command(tool, directory, common, suffix=suffix, manifest_tool=manifest_tool, apply=apply)
    return await asyncio.to_thread(subprocess.run, command, cwd=ROOT, env=environment,
                                   capture_output=True, text=True)


async def require_committed_chain(directory, common, environment, *, copy=True):
    """Run the real bootstrap (and copy) CLI so a later tool sees committed state."""
    result = await invoke('bootstrap', directory, common, environment)
    assert result.returncode == 0, result.stderr
    common['bootstrap_receipt_sha256'] = hashlib.sha256(
        Path(common['bootstrap_receipt']).read_bytes()).hexdigest()
    if copy:
        result = await invoke('copy', directory, common, environment)
        assert result.returncode == 0, result.stderr
    return common


async def start_tool(tool, directory, common, environment, *, suffix='', apply=True):
    """Start the tool without waiting, so a test can inject an in-flight fault."""
    command = tool_command(tool, directory, common, suffix=suffix, apply=apply)
    return await asyncio.create_subprocess_exec(
        *command, cwd=ROOT, env=environment,
        stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)


async def finish_tool(process):
    """Wait for a start_tool process and collect its complete output."""
    stdout, stderr = await process.communicate()
    return ToolResult(process.returncode, stdout.decode(), stderr.decode())


async def wait_for_advisory_lock(scratch, process, key, *, timeout=120):
    """Wait until the running tool holds its transaction-scoped advisory lock.

    Bootstrap and copy take that lock after their entry boundary and hold it
    until the mutation ends. A fault injected once the lock is held can only be
    refused by a later boundary, never by the entry observation.
    """
    deadline = time.monotonic() + timeout
    async with scratch.admin_connection() as admin:
        while time.monotonic() < deadline:
            if process.returncode is not None:
                result = await finish_tool(process)
                raise AssertionError(
                    f'Tool exited {result.returncode} before taking lock {key}: {result.stderr}')
            free = await admin.fetchval('SELECT pg_try_advisory_xact_lock($1::bigint)', key)
            if not free:
                return
            await asyncio.sleep(0.1)
    raise AssertionError(f'Tool never took advisory lock {key}')


async def wait_for_second_source_transaction(scratch, process, *, timeout=180):
    """Wait until the tool's read-only source session opens its second transaction.

    Every tool observes the entry boundary in one short transaction. verify then
    opens its long repeatable-read transaction on the same session. Once that
    second transaction exists, a committed change is invisible to its snapshot
    and can only be refused by a later boundary or the final fresh observation.
    """
    deadline = time.monotonic() + timeout
    seen = set()
    async with scratch.admin_connection() as admin:
        while time.monotonic() < deadline:
            if process.returncode is not None:
                result = await finish_tool(process)
                raise AssertionError(
                    f'Tool exited {result.returncode} before its second source transaction: '
                    f'{result.stderr}')
            row = await admin.fetchrow('''
                SELECT pid::text AS pid, xact_start::text AS started
                FROM pg_stat_activity
                WHERE datname='verdaxis' AND usename='verdaxis_backup'
            ''')
            if row is not None and row['started'] is not None:
                seen.add((row['pid'], row['started']))
                if len(seen) >= 2:
                    # Let the snapshot's first read complete before injecting.
                    await asyncio.sleep(0.5)
                    return
            await asyncio.sleep(0.1)
    raise AssertionError('Tool never opened a second source transaction')


async def late_source_write(scratch):
    """Commit one source row change, exactly as a late external writer would."""
    async with scratch.admin_connection('verdaxis') as admin:
        tag = await admin.execute("UPDATE notifications SET title='late write' WHERE title IS NOT NULL")
    changed = int(tag.split()[-1])
    assert changed > 0, tag
    return changed


async def bump_source_sequence(scratch):
    """Advance one source sequence.

    Sequence state is not MVCC-snapshot bound, so a fence that reads it inside a
    repeatable-read transaction still sees this change. PostgreSQL was verified
    to behave this way on the pinned PostgreSQL 17 test image.
    """
    async with scratch.admin_connection('verdaxis') as admin:
        name = await admin.fetchval("""
            SELECT c.oid::regclass::text FROM pg_class c
            JOIN pg_namespace n ON n.oid=c.relnamespace
            WHERE c.relkind='S' AND n.nspname='public' ORDER BY c.relname LIMIT 1
        """)
        current = await admin.fetchval(f'SELECT last_value FROM {name}')
        changed = await admin.fetchval('SELECT setval(CAST($1 AS regclass), CAST($2 AS bigint))',
                                       name, current + 1000003)
    assert changed == current + 1000003
    return changed


async def grant_target_connect(scratch, role='verdaxis_app'):
    """Publish a late target CONNECT grant; the tools must refuse it."""
    async with scratch.admin_connection() as admin:
        await admin.execute(f'GRANT CONNECT ON DATABASE verdaxis_prod TO {role}')
