"""Converge canonical ACLs only on a copied disposable union database."""
import os
import subprocess
from pathlib import Path

import asyncpg
import pytest
from sqlalchemy import text

from scripts.phase0.copy import copy_database
from tests.phase0.test_copy import copy_case as copy_case

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.asyncio
async def test_copied_union_runtime_and_backup_denials(copy_case):
    manifest, source, target, admin = copy_case
    await copy_database(manifest, source, target)
    identity = manifest['execution']['target']
    suffix = identity['database'].removeprefix('phase0_target_')
    app, backup = 'phase0_app_' + suffix, 'phase0_reader_' + suffix
    environment = {'PATH': os.environ['PATH'], 'PGHOST': '127.0.0.1',
                   'PGPORT': str(identity['port']), 'PGUSER': 'postgres',
                   'PGPASSWORD': os.environ['PHASE0_PASSWORD'], 'PGDATABASE': identity['database']}
    arguments = ['psql', '-X', '-v', 'ON_ERROR_STOP=1', '-v', 'database_name=' + identity['database'],
                 '-v', 'app_role=' + app, '-v', 'migrator_role=phase0_owner', '-v', 'backup_role=' + backup]
    try:
        for filename in ('bootstrap_roles.sql', 'validate_roles.sql'):
            result = subprocess.run([*arguments, '-f', str(ROOT / 'deploy/postgres' / filename)],
                                    env=environment, capture_output=True, text=True)
            assert result.returncode == 0, result.stderr
        for role in (app, backup):
            await admin.execute(f"ALTER ROLE {role} PASSWORD '{os.environ['PHASE0_PASSWORD']}'")
            connection = await asyncpg.connect(host='127.0.0.1', port=identity['port'],
                                               user=role, password=os.environ['PHASE0_PASSWORD'],
                                               database=identity['database'])
            try:
                for statement in (
                    'UPDATE kyc_execution_grace SET created_by=created_by WHERE false',
                    'UPDATE market_command_results SET request_hash=request_hash WHERE false',
                    'UPDATE audit_logs SET action=action WHERE false',
                    'UPDATE organizations SET provenance=provenance WHERE false',
                    'DELETE FROM kyc_documents WHERE false',
                    'UPDATE alembic_version SET version_num=version_num WHERE false',
                    'UPDATE seed_runs SET id=id WHERE false',
                    'CREATE TABLE public.import_probe (id integer)',
                ):
                    with pytest.raises(asyncpg.InsufficientPrivilegeError):
                        await connection.execute(statement)
                assert await connection.fetchval('SELECT count(*) FROM trades') == 9
            finally:
                await connection.close()
        assert (await target.execute(text("SELECT count(*) FROM pg_roles WHERE rolname=:role AND (rolsuper OR rolbypassrls)"),
                                     {'role': app})).scalar_one() == 0
        await target.rollback()
    finally:
        # Remove these roles' local grants before the shared fixture drops its DB.
        for role in (app, backup):
            exists = await admin.fetchval('SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname=$1)', role)
            if exists:
                subprocess.run([*arguments, '-c', f'DROP OWNED BY {role}'],
                               env=environment, check=True, capture_output=True)
                await admin.execute(f'DROP ROLE {role}')
