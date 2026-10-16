"""One-use production authority for the Phase 0 tools, never a generic override.

The operator installs the fixed root-private authority from independently
reviewed release and database observations before running any tool. Commands
cannot supply or replace this authority. No grants or host configuration change.
"""
from __future__ import annotations

import hashlib
import os
import re
from pathlib import Path

from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.ext.asyncio import create_async_engine
from sqlalchemy.pool import NullPool

from scripts.phase0.database_fence import check_source_quiesced, check_target_fence
from scripts.phase0.host_authority import (
    parse_json,
    read_root_private,
    tree_digest,
    validate_identity,
)
from scripts.phase0.refusal import Refusal
from scripts.phase0.manifest import (
    PROD_SHA,
    digest,
    inspect_catalog,
    load_manifest,
    require_complete_inventory,
)

ROOT = Path(__file__).resolve().parents[2]
AUTHORITY_PATH = Path('/root/verdaxis-phase0/authority.json')
COMMON_FIELDS = {
    'mode', 'approved', 'hostname', 'release_sha', 'tool_tree_sha256', 'source_sha',
    'source', 'target', 'source_catalog_sha256', 'frozen_source', 'target_datacl',
    'bootstrap_receipt',
}
MANIFEST_FIELDS = COMMON_FIELDS | {'tool', 'report', 'bootstrap_receipt_sha256'}


def private_output(path):
    path = Path(path)
    if not path.is_absolute() or '..' in path.parts:
        raise ValueError('Absolute evidence path required')
    for parent in path.parents:
        info = parent.lstat()
        if parent.is_symlink() or not parent.is_dir() or info.st_uid != 0 or info.st_mode & 0o022:
            raise ValueError('Evidence path is not root controlled')
    if path.parent.stat().st_mode & 0o777 != 0o700:
        raise ValueError('Evidence directory must be private')
    return path


def load_authority(manifest, tool):
    if os.geteuid() != 0 or os.environ.get('ENVIRONMENT') != 'production':
        raise Refusal('PHASE0_ENVIRONMENT')
    if set(manifest) != MANIFEST_FIELDS or manifest['tool'] != tool:
        raise Refusal('PHASE0_MANIFEST')
    authority = parse_json(read_root_private(AUTHORITY_PATH))
    if set(authority) != COMMON_FIELDS or any(manifest[key] != authority[key] for key in COMMON_FIELDS):
        raise Refusal('PHASE0_AUTHORITY')
    if authority['mode'] != 'production' or authority['approved'] is not True:
        raise Refusal('PHASE0_AUTHORITY')
    if authority['hostname'] != os.uname().nodename or authority['source_sha'] != PROD_SHA:
        raise Refusal('PHASE0_AUTHORITY')
    if (not isinstance(authority['release_sha'], str)
            or not re.fullmatch('[0-9a-f]{40}', authority['release_sha'])
            or authority['release_sha'] != os.environ.get('RELEASE_SHA')):
        raise Refusal('PHASE0_AUTHORITY')
    for side in ('source', 'target'):
        validate_identity(authority[side], side=side)
    source, target = authority['source'], authority['target']
    if source['oid'] == target['oid'] or any(source[key] != target[key] for key in
            ('host', 'port', 'server_host', 'server_port', 'cluster_system_identifier')):
        raise Refusal('PHASE0_DB_PAIR')
    if authority['source_catalog_sha256'] != digest(load_manifest()['source_catalog']):
        raise Refusal('PHASE0_AUTHORITY')
    # The operator attests the archived release bytes, not mutable Git metadata.
    # Every source directory/file must also be root-controlled at execution.
    for path in [ROOT, *ROOT.parents]:
        info = path.lstat()
        if path.is_symlink() or info.st_uid != 0 or info.st_mode & 0o022:
            raise Refusal('PHASE0_INSTALLED_BYTES')
    for name in ('scripts', 'app', 'alembic', 'deploy'):
        for path in (ROOT / name).rglob('*'):
            if '__pycache__' in path.parts:
                continue
            info = path.lstat()
            if path.is_symlink() or info.st_uid != 0 or info.st_mode & 0o022:
                raise Refusal('PHASE0_INSTALLED_BYTES')
    if authority['tool_tree_sha256'] != tree_digest(ROOT):
        raise Refusal('PHASE0_INSTALLED_BYTES')
    from scripts.phase0.bootstrap import load_policy
    load_policy()
    private_output(authority['bootstrap_receipt'])
    output = private_output(manifest['report'])
    if os.path.lexists(output):
        raise Refusal('PHASE0_REPORT_PATH')
    if tool == 'bootstrap':
        if manifest['bootstrap_receipt_sha256'] is not None or str(output) != authority['bootstrap_receipt']:
            raise Refusal('PHASE0_RECEIPT')
    else:
        receipt_bytes = read_root_private(authority['bootstrap_receipt'])
        if hashlib.sha256(receipt_bytes).hexdigest() != manifest['bootstrap_receipt_sha256']:
            raise Refusal('PHASE0_RECEIPT')
        receipt = parse_json(receipt_bytes)
        if receipt['authority_digest'] != digest(authority) or receipt['target_identity'] != target:
            raise Refusal('PHASE0_RECEIPT')
    return authority


def secret_url(path, identity):
    contents = read_root_private(path).decode()
    lines = contents.splitlines()
    if len(lines) != 1 or not lines[0].startswith('DATABASE_URL='):
        raise ValueError('One explicit DATABASE_URL required')
    url = make_url(lines[0][len('DATABASE_URL='):])
    if (url.drivername != 'postgresql+asyncpg' or url.query or not url.password
            or (url.host, url.port, url.database, url.username) != tuple(
                identity[key] for key in ('host', 'port', 'database', 'role'))):
        raise ValueError('Secret endpoint differs')
    return url


async def attest_endpoint(connection, identity):
    row = (await connection.execute(text('''
        SELECT current_database() AS database, current_user AS role, session_user,
               d.oid, inet_server_addr()::text AS server_host, inet_server_port() AS server_port,
               (SELECT system_identifier::text FROM pg_control_system()) AS cluster_system_identifier
        FROM pg_database d WHERE d.datname=current_database()
    '''))).mappings().one()
    if row['session_user'] != identity['role'] or any(row[key] != identity[key] for key in
            ('database', 'role', 'oid', 'server_host', 'server_port', 'cluster_system_identifier')):
        raise Refusal('PHASE0_DB_IDENTITY')


async def check_boundaries(authority, source, target, *, frozen=True):
    from scripts.phase0.verify import fingerprint_database
    await attest_endpoint(source, authority['source'])
    await attest_endpoint(target, authority['target'])
    await check_source_quiesced(source, authority['source'])
    await check_target_fence(target, authority['target'], authority['target_datacl'])
    if frozen:
        require_complete_inventory(await source.run_sync(inspect_catalog), load_manifest()['source_catalog'])
        if await fingerprint_database(source, load_manifest()['source_catalog']) != authority['frozen_source']:
            raise Refusal('PHASE0_SOURCE_STATE')


def copy_contract(authority, manifest):
    receipt = parse_json(read_root_private(authority['bootstrap_receipt']))
    contract = load_manifest()
    contract['execution'] = dict(
        mode='production', host_id=authority['hostname'], source=authority['source'], target=authority['target'],
        baseline_reference_rows=receipt['baseline_reference_rows'], baseline_sequences=receipt['baseline_sequences'],
        frozen_source=authority['frozen_source'], source_catalog_sha256=authority['source_catalog_sha256'],
        receipt_path=manifest['report'], production_manifest=manifest,
    )
    return contract


def require_tool(manifest, expected_tool):
    if expected_tool not in ('bootstrap', 'copy', 'verify') or manifest.get('tool') != expected_tool:
        raise Refusal('PHASE0_TOOL_MISMATCH')


async def run(manifest, source_file, target_file, *, expected_tool, apply=False):
    from scripts.phase0.codecs import configure_text_session
    from scripts.phase0.copy import attest, copy_database
    from scripts.phase0.verify import (
        publish_report,
        verify_independent,
    )

    require_tool(manifest, expected_tool)
    tool = expected_tool
    authority = load_authority(manifest, tool)
    if tool != 'verify' and not apply:
        # Validation only: no connection, no secret read, no published evidence.
        return dict(validated_only=True)
    urls = [secret_url(path, authority[side]) for path, side in
            ((source_file, 'source'), (target_file, 'target'))]
    if urls[0].password == urls[1].password:
        raise ValueError('Distinct source and target credentials required')
    engines = [create_async_engine(url, poolclass=NullPool, hide_parameters=True,
               connect_args={'server_settings': {'default_transaction_read_only': 'on'}}
               if index == 0 or tool == 'verify' else {}) for index, url in enumerate(urls)]
    try:
        async with engines[0].connect() as source, engines[1].connect() as target:
            if tool == 'bootstrap':
                from alembic.config import Config

                from scripts.phase0.bootstrap import (
                    BootstrapIdentity,
                    bootstrap_connection,
                )
                async with source.begin():
                    await source.execute(text('SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY'))
                    await configure_text_session(source)
                    await attest(source, authority['source'], owner=False)
                    await check_boundaries(authority, source, target)
                    await target.rollback()
                    identity = BootstrapIdentity(**authority['target'], source_sha=authority['release_sha'],
                                                 production_manifest=manifest)
                    config = Config(str(ROOT / 'alembic.ini'))
                    config.set_main_option('script_location', str(ROOT / 'alembic'))
                    baseline = await bootstrap_connection(target, identity, config, apply=True,
                        boundary=lambda: check_boundaries(authority, source, target))
                    await check_boundaries(authority, source, target)
                    await target.rollback()
                report = dict(authority_digest=digest(authority), target_identity=authority['target'], **baseline)
            elif tool == 'copy':
                # copy_database performs its own boundary checks and publishes the
                # private receipt only after commit.
                await copy_database(copy_contract(authority, manifest), source, target)
                return dict(validated_only=False)
            else:
                report = await verify_independent(copy_contract(authority, manifest), source, target)
            # Fresh transactions ensure the final source observation is not the
            # original MVCC snapshot. Report publication is after this check.
            async with source.begin(), target.begin():
                await source.execute(text('SET TRANSACTION READ ONLY'))
                await target.execute(text('SET TRANSACTION READ ONLY'))
                await configure_text_session(source)
                await configure_text_session(target)
                await check_boundaries(authority, source, target)
            publish_report(manifest['report'], report)
            return dict(validated_only=False)
    finally:
        for engine in engines:
            await engine.dispose()
