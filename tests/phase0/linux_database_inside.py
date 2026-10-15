"""Container-local synthetic setup; never accepts host URLs or real data."""
import asyncio
import json
import os
import subprocess
from pathlib import Path

import asyncpg

ROOT = Path('/app')
WORK = Path('/phase0-work')


async def provision(inputs):
    connection = await asyncpg.connect(host='127.0.0.1', port=inputs['port'],
                                      user='postgres', password=inputs['password'], database='postgres')
    try:
        cluster = await connection.fetchval('SELECT system_identifier::text FROM pg_control_system()')
        if cluster != inputs['cluster']:
            raise ValueError('Runner cluster identity differs')
        password = inputs['password']
        if len(password) != 64 or any(character not in '0123456789abcdef' for character in password):
            raise ValueError('Generated scratch credential required')
        await connection.execute(f"CREATE ROLE phase0_owner LOGIN NOINHERIT PASSWORD '{password}'")
        for database in ('phase0_prod_test', 'phase0_staging_test'):
            await connection.execute(f'CREATE DATABASE {database} OWNER phase0_owner')
    finally:
        await connection.close()
    for database in ('template1', 'phase0_prod_test', 'phase0_staging_test'):
        connection = await asyncpg.connect(host='127.0.0.1', port=inputs['port'],
                                          user='postgres', password=inputs['password'], database=database)
        try:
            await connection.execute('CREATE EXTENSION IF NOT EXISTS postgis')
        finally:
            await connection.close()


def main():
    inputs = json.loads(Path('/inputs.json').read_text())
    if inputs['port'] != 55432:
        raise ValueError('Fixed container-local endpoint required')
    WORK.mkdir(mode=0o700)
    environment = dict(PATH=os.environ['PATH'], HOME='/root', ENVIRONMENT='test', RELEASE_SHA='test',
                       JWT_SECRET='test-secret-key-for-testing-minimum-32-chars',
                       PYTHONDONTWRITEBYTECODE='1', KYC_EXECUTION_GATE_ENABLED='false',
                       PHASE0_PORT=str(inputs['port']), PHASE0_PASSWORD=inputs['password'],
                       PHASE0_CLUSTER_ID=inputs['cluster'], PHASE0_WORK=str(WORK))
    os.environ.clear()
    os.environ.update(environment)
    (ROOT / 'venv').symlink_to('/usr/local', target_is_directory=True)
    asyncio.run(provision(inputs))
    for name, revision in (('prod', 'miq_20260720_market_quarantine'),
                           ('staging', 'kycd_20261015_staging')):
        tree = WORK / name
        tree.mkdir()
        subprocess.run(['tar', '-xf', '/' + name + '.tar', '-C', str(tree)], check=True)
        (tree / 'venv').symlink_to('/usr/local', target_is_directory=True)
        url = f"postgresql+asyncpg://phase0_owner:{inputs['password']}@127.0.0.1:{inputs['port']}/phase0_{name}_test"
        subprocess.run(['/usr/local/bin/python', '-m', 'alembic', 'upgrade', revision],
                       cwd=tree, env=dict(environment, DATABASE_URL=url, MIGRATOR_DATABASE_URL=url), check=True)
    subprocess.run(['/usr/local/bin/python', '-m', 'tests.phase0.build_fixtures'], cwd=ROOT, check=True)
    return subprocess.run(['/usr/local/bin/python', '-m', 'pytest', '-q',
                           'tests/phase0/test_production_scratch.py'], cwd=ROOT).returncode


if __name__ == '__main__':
    raise SystemExit(main())
