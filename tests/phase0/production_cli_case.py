"""Root-container-only inputs for the real production CLI acceptance test."""
import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path

from sqlalchemy.ext.asyncio import create_async_engine
from sqlalchemy.pool import NullPool

from scripts.phase0.host_authority import tree_digest
from scripts.phase0.manifest import digest, load_manifest
from scripts.phase0.verify import fingerprint_database

ROOT = Path('/app')


def private_file(path, content):
    path.write_text(content)
    path.chmod(0o600)


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
    authority_directory = Path('/root/verdaxis-phase0')
    authority_directory.mkdir(mode=0o700, exist_ok=True)
    authority = {key: value for key, value in common.items() if key != 'bootstrap_receipt_sha256'}
    private_file(authority_directory / 'authority.json', json.dumps(authority))
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
    return common, environment


async def invoke(tool, directory, common, environment, *, suffix=''):
    manifest = dict(common, tool=tool, report=str(directory / f'{tool}{suffix}-report.json'))
    path = directory / f'{tool}{suffix}.json'
    private_file(path, json.dumps(manifest))
    command = [sys.executable, '-m', f'scripts.phase0.{tool}', '--manifest', str(path),
               '--source-environment-file', str(directory / f'{tool}-source.env'),
               '--target-environment-file', str(directory / f'{tool}-target.env')]
    if tool == 'bootstrap':
        command += ['--apply']
    elif tool == 'copy':
        command += ['--source-sha', common['source_sha'], '--target-sha', common['release_sha'], '--apply']
    else:
        command += ['--report', manifest['report']]
    return await asyncio.to_thread(subprocess.run, command, cwd=ROOT, env=environment,
                                   capture_output=True, text=True)
