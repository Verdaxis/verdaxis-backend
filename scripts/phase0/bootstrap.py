"""One-use union bootstrap. Host apply remains closed pending host integration."""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import json
import os
import re
import subprocess
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from alembic.config import Config
from sqlalchemy import text
from sqlalchemy.engine import Connection, make_url
from sqlalchemy.ext.asyncio import create_async_engine
from sqlalchemy.pool import NullPool

from alembic import command
from scripts.phase0.copy import read_private_file
from scripts.phase0.manifest import digest, inspect_catalog

ROOT = Path(__file__).resolve().parents[2]
POLICY_PATH = ROOT / 'deploy/phase0/bootstrap-policy.json'
# One transaction-scoped lock per database, shared by all bootstrap callers.
BOOTSTRAP_LOCK = 706016004


@dataclass(frozen=True)
class BootstrapIdentity:
    database: str
    oid: int
    role: str
    host: str
    port: int
    server_host: str
    server_port: int
    cluster_system_identifier: str
    source_sha: str
    production_manifest: dict | None = None

    def validate(self):
        if self.production_manifest is not None:
            from scripts.phase0.production import load_authority
            authority = load_authority(self.production_manifest, 'bootstrap')
            if self.source_sha != authority['release_sha'] or any(
                    getattr(self, key) != value for key, value in authority['target'].items()):
                raise ValueError('Bootstrap production identity differs')
            return
        environment = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
        actual_sha = subprocess.check_output(
            ['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], env=environment, text=True,
        ).strip()
        if not re.fullmatch(r'[0-9a-f]{40}', self.source_sha) or self.source_sha != actual_sha:
            raise ValueError('Bootstrap approved source SHA differs')
        if (self.host != '127.0.0.1' or type(self.port) is not int
                or not 1024 < self.port < 65536
                or self.port != int(os.environ.get('PHASE0_PORT', '0'))):
            raise ValueError('Disposable bootstrap endpoint required')
        if (not self.database.startswith('phase0_bootstrap_')
                or type(self.oid) is not int or self.oid <= 0
                or self.role != 'phase0_owner'):
            raise ValueError('Synthetic bootstrap database identity required')
        # The container-owning runner supplies the independent cluster binding.
        # Bootstrap grants no privileges to read control data.
        if (not self.cluster_system_identifier.isdigit()
                or self.cluster_system_identifier != os.environ.get('PHASE0_CLUSTER_ID')):
            raise ValueError('Trusted bootstrap cluster binding required')


def migration_graph_digest():
    files = sorted((ROOT / 'alembic/versions').glob('*.py'))
    return digest({p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files})


def load_policy():
    policy = json.loads(POLICY_PATH.read_text())
    if (policy['mode'] != 'synthetic' or policy['host_apply'] is not False
            or policy['target_revision'] != 'p0_20261016_union'
            or policy['migration_graph_sha256'] != migration_graph_digest()):
        raise ValueError('Bootstrap policy or migration graph differs')
    return policy


def attest_bootstrap_connection(connection, identity):
    if not isinstance(identity, BootstrapIdentity):
        raise ValueError('Validated bootstrap identity required')
    identity.validate()
    load_policy()
    if not isinstance(connection, Connection) or not connection.in_transaction():
        raise ValueError('Bootstrap must own an outer transaction')
    url = connection.engine.url
    if (url.query or url.get_backend_name() != 'postgresql'
            or (url.host, url.port, url.database, url.username) !=
            (identity.host, identity.port, identity.database, identity.role)):
        raise ValueError('Bootstrap connection endpoint differs')
    # SQLAlchemy's logical transaction can exist while the driver autocommits.
    # Start the driver's lazy transaction with a read, then require PostgreSQL's
    # physical transaction state before attestation, lock acquisition or DDL.
    # Only asyncpg is supported by this bootstrap; unknown drivers fail closed.
    if connection.dialect.driver != 'asyncpg':
        raise ValueError('Bootstrap requires an asyncpg physical transaction')
    connection.execute(text('SELECT 1'))
    driver = connection.connection.driver_connection
    if not driver.is_in_transaction():
        raise ValueError('Bootstrap requires a physical transaction; AUTOCOMMIT refused')
    row = connection.execute(text('''
        SELECT current_database() AS database, current_user AS role, session_user,
               d.oid, inet_server_addr()::text AS server_host,
               inet_server_port() AS server_port, r.rolsuper, r.rolcreatedb,
               r.rolcreaterole, r.rolreplication, r.rolbypassrls, r.rolinherit,
               r.rolcanlogin, d.datdba=r.oid AS owns_database,
               EXISTS (SELECT 1 FROM pg_auth_members WHERE member=r.oid) AS memberships
        FROM pg_database d JOIN pg_roles r ON r.rolname=current_user
        WHERE d.datname=current_database()
    ''')).mappings().one()
    cluster_id = connection.execute(text(
        'SELECT system_identifier::text FROM pg_control_system()'
    )).scalar_one()
    if cluster_id != identity.cluster_system_identifier:
        raise ValueError('Bootstrap connected cluster differs')
    if any(row[key] != getattr(identity, key) for key in
           ('database', 'role', 'oid', 'server_host', 'server_port')):
        raise ValueError('Bootstrap database identity differs')
    if (row['session_user'] != identity.role or not row['owns_database']
            or not row['rolcanlogin'] or any(row[key] for key in
            ('rolsuper', 'rolcreatedb', 'rolcreaterole', 'rolreplication',
             'rolbypassrls', 'rolinherit', 'memberships'))):
        raise ValueError('Bootstrap role authority refused')
    if not connection.execute(text('SELECT pg_try_advisory_xact_lock(:key)'),
                              {'key': BOOTSTRAP_LOCK}).scalar_one():
        raise ValueError('Bootstrap lock is busy')
    # Extension objects from the administrator-provisioned template are allowed.
    count = connection.execute(text('''
        SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
        WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND c.relkind IN ('r', 'p', 'S', 'v', 'm', 'f')
          AND n.nspname NOT LIKE 'pg_toast%'
          AND NOT EXISTS (SELECT 1 FROM pg_depend d
              WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e')
    ''')).scalar_one()
    if count:
        raise ValueError('Bootstrap requires an empty database')


@contextmanager
def refuse_autocommit():
    raise ValueError('Bootstrap forbids an autocommit escape')
    yield  # pragma: no cover


def verify_bootstrap_schema(connection, policy):
    catalog = inspect_catalog(connection)
    structure = {key: value for key, value in catalog.items() if key != 'reference_rows'}
    if catalog['revision'] != policy['target_revision'] or digest(structure) != policy['schema_sha256']:
        raise ValueError('Bootstrap schema verification failed')
    reference_values = {
        table: [{key: value for key, value in row.items() if key != 'created_at'} for row in rows]
        for table, rows in catalog['reference_rows'].items()
    }
    if digest(reference_values) != policy['reference_values_sha256']:
        raise ValueError('Bootstrap reference baseline verification failed')
    for table in catalog['tables']:
        if table not in ('alembic_version', 'products', 'delivery_points'):
            quoted = table.replace('"', '""')
            if connection.execute(text(f'SELECT EXISTS (SELECT 1 FROM "{quoted}")')).scalar_one():
                raise ValueError('Bootstrap business rows are not empty')


async def bootstrap_connection(connection, identity, config, *, apply=False, boundary=None):
    if apply is not True:
        raise ValueError('Explicit bootstrap approval required')
    identity.validate()
    policy = load_policy()
    if (Path(config.get_main_option('script_location')).resolve() != ROOT / 'alembic'
            or config.get_main_option('version_locations')):
        raise ValueError('Bootstrap requires the reviewed migration directory')
    if connection.in_transaction():
        raise ValueError('Bootstrap caller already has a transaction')
    async with connection.begin():
        def upgrade(sync):
            if any(key in config.attributes for key in
                   ('connection', 'phase0_bootstrap_identity', 'phase0_bootstrap_approved')):
                raise ValueError('Bootstrap Config attributes are already occupied')
            config.attributes.update(connection=sync, phase0_bootstrap_identity=identity,
                                     phase0_bootstrap_approved=True)
            try:
                command.upgrade(config, policy['target_revision'])
                verify_bootstrap_schema(sync, policy)
            finally:
                config.attributes.pop('connection', None)
                config.attributes.pop('phase0_bootstrap_identity', None)
                config.attributes.pop('phase0_bootstrap_approved', None)
        await connection.run_sync(upgrade)
        if boundary is not None:
            await boundary()
        from scripts.phase0.verify import sequence_states
        catalog = await connection.run_sync(inspect_catalog)
        baseline = dict(baseline_reference_rows=catalog['reference_rows'],
                        baseline_sequences=await sequence_states(connection, catalog))
    return baseline


def validate_manifest(manifest):
    if set(manifest) != {'mode', 'approved', 'target_revision', 'identity'}:
        raise ValueError('Invalid bootstrap manifest structure')
    policy = load_policy()
    if (manifest['mode'] != 'synthetic' or manifest['approved'] is not True
            or manifest['target_revision'] != policy['target_revision']):
        raise ValueError('Bootstrap approval or literal target refused')
    identity = BootstrapIdentity(**manifest['identity'])
    identity.validate()
    return identity


async def apply_manifest(identity, environment_file):
    # This private file has one explicit key; ambient Settings cannot route DDL.
    contents = read_private_file(environment_file).strip()
    prefix = 'MIGRATOR_DATABASE_URL='
    if not contents.startswith(prefix) or '\n' in contents:
        raise ValueError('Explicit target environment file required')
    url = make_url(contents[len(prefix):])
    if (url.drivername != 'postgresql+asyncpg' or url.query or not url.password
            or (url.host, url.port, url.database, url.username) !=
            (identity.host, identity.port, identity.database, identity.role)):
        raise ValueError('Target environment endpoint differs')
    engine = create_async_engine(url, poolclass=NullPool, hide_parameters=True)
    config = Config(str(ROOT / 'alembic.ini'))
    config.set_main_option('script_location', str(ROOT / 'alembic'))
    try:
        async with engine.connect() as connection:
            await bootstrap_connection(connection, identity, config, apply=True)
    finally:
        await engine.dispose()


def main():
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--target-environment-file', type=Path, required=True)
    parser.add_argument('--source-environment-file', type=Path)
    parser.add_argument('--apply', action='store_true')
    args = parser.parse_args()
    try:
        manifest = json.loads(read_private_file(args.manifest))
        if manifest.get('mode') == 'production':
            from scripts.phase0.host_authority import parse_json, read_root_private
            from scripts.phase0.production import run
            manifest = parse_json(read_root_private(args.manifest))
            if args.source_environment_file is None:
                raise ValueError('Production source environment file required')
            asyncio.run(run(manifest, args.source_environment_file, args.target_environment_file, apply=args.apply))
            print('Production bootstrap completed')
            return
        identity = validate_manifest(manifest)
        if args.apply:
            asyncio.run(apply_manifest(identity, args.target_environment_file))
        print('Bootstrap applied' if args.apply else 'Manifest checked; no connection opened or DDL approved')
    except Exception:
        parser.exit(1, 'Bootstrap refused; no credentials or SQL details are printed\n')


if __name__ == '__main__':
    # env.py must see the same dataclass type when this module runs with -m.
    from scripts.phase0.bootstrap import main as bootstrap_main

    bootstrap_main()
