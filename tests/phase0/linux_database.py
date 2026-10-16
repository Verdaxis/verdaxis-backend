"""Own an offline Linux/PostgreSQL boundary for synthetic production-name tests."""
from __future__ import annotations

import json
import secrets
import subprocess
import tempfile
import time
from pathlib import Path
from uuid import uuid4

from tests.phase0.linux_hosttools import IMAGE, PLATFORM, ROOT
from tests.phase0.runner import IMAGE as POSTGRES_IMAGE

ARCHIVES = {
    'prod': 'c08266bd6969143812dec7c5033338428cd5f159',
    'staging': 'b218ab264b92f99007bacc05d4871413e024b72a',
    'candidate': 'HEAD',
}
PORT = 55432


def database_command(name, password):
    return ['docker', 'run', '-d', '--name', name, '--platform', 'linux/amd64',
            '--network', 'none', '-e', 'POSTGRES_PASSWORD=' + password,
            POSTGRES_IMAGE, 'postgres', '-c', 'port=' + str(PORT), '-c', 'listen_addresses=127.0.0.1']


def runtime_command(name, database):
    return ['docker', 'create', '--name', name, '--platform', PLATFORM,
            '--network', 'container:' + database, '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges',
            IMAGE, '/bin/sh', '-ec',
            'tar -xf /candidate.tar -C /app; python -m tests.phase0.linux_database_inside']


def checked(arguments, **kwargs):
    return subprocess.run(arguments, check=True, **kwargs)


def main():
    token = uuid4().hex[:12]
    database, runtime = 'phase0-linux-db-' + token, 'phase0-linux-run-' + token
    password = secrets.token_hex(32)
    created = []
    work = ROOT / 'work'
    work.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='linux-database-', dir=work) as temporary:
        directory = Path(temporary)
        try:
            checked(database_command(database, password), stdout=subprocess.DEVNULL)
            created.append(database)
            for attempt in range(240):
                ready = subprocess.run(['docker', 'exec', database, 'pg_isready',
                                        '-h', '127.0.0.1', '-p', str(PORT), '-U', 'postgres'],
                                       capture_output=True)
                if ready.returncode == 0:
                    break
                time.sleep(1)
            else:
                raise RuntimeError('Owned PostgreSQL did not become ready')
            cluster = checked(['docker', 'exec', database, 'psql', '-XAt', '-U', 'postgres',
                               '-p', str(PORT), '-c', 'SELECT system_identifier FROM pg_control_system()'],
                              capture_output=True, text=True).stdout.strip()
            checked(runtime_command(runtime, database), stdout=subprocess.DEVNULL)
            created.append(runtime)
            for name, revision in ARCHIVES.items():
                archive = directory / (name + '.tar')
                with archive.open('wb') as stream:
                    checked(['git', '-C', str(ROOT), 'archive', revision], stdout=stream)
                checked(['docker', 'cp', str(archive), runtime + ':/' + archive.name])
            release = directory / 'release.json'
            release.write_text(json.dumps(dict(release_sha=checked(
                ['git', '-C', str(ROOT), 'rev-parse', 'HEAD'], capture_output=True, text=True,
            ).stdout.strip())))
            release.chmod(0o600)
            checked(['docker', 'cp', str(release), runtime + ':/release.json'])
            inputs = directory / 'inputs.json'
            inputs.write_text(json.dumps(dict(port=PORT, password=password, cluster=cluster)))
            inputs.chmod(0o600)
            checked(['docker', 'cp', str(inputs), runtime + ':/inputs.json'])
            result = subprocess.run(['docker', 'start', '-a', runtime])
            status = checked(['docker', 'inspect', '--format', '{{.State.ExitCode}}', runtime],
                             capture_output=True, text=True)
            return result.returncode or int(status.stdout.strip())
        finally:
            for container in reversed(created):
                checked(['docker', 'rm', '-f', '-v', container])


if __name__ == '__main__':
    raise SystemExit(main())
