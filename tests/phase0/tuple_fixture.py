"""Test-only executable host model. Never controls real services or databases.

The loaded-unit file models systemd's cache; it is separate from unit bytes.
The start adapter invokes both real guards before a release-specific executable.
"""
import json
import os
import shutil
import sqlite3
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OLD_SHA = 'b' * 40
NEW_SHA = 'a' * 40

LOCK_HOLDER = """
import fcntl, json, os, pathlib, sys
control, root, sha, database = sys.argv[1:]
control, root = pathlib.Path(control), pathlib.Path(root)
handles = []
for path in (control / 'operator.lock', root / '.runtime-deploy/production.lock',
             root / '.runtime-deploy/production.restart.lock'):
    handle = path.open('w+')
    os.chmod(path, 0o600)
    fcntl.flock(handle, fcntl.LOCK_EX)
    handles.append(handle)
info = os.fstat(handles[0].fileno())
permit = control / 'permit.json'
permit.write_text(json.dumps(dict(environment='production', release_sha=sha,
    database=database, lock_device=info.st_dev, lock_inode=info.st_ino)))
permit.chmod(0o600)
(root / '.runtime-deploy/production.state').write_text(
    'DEPLOYMENT_STATE=restart-authorized\\nENVIRONMENT=production\\nRELEASE_SHA=' + sha + '\\n')
(root / '.runtime-deploy/production.state').chmod(0o600)
print('locked', flush=True)
sys.stdin.read()
"""

START_ADAPTER = """
import importlib.util, json, os, pathlib, subprocess, sys
workspace, source = map(pathlib.Path, sys.argv[1:])
sys.path.insert(0, str(source))
from scripts.phase0.start_fence import start_is_allowed
root = workspace / 'be'
unit = json.loads((workspace / 'loaded-unit').read_text())
try:
    release = dict(line.split('=', 1) for line in (root / '.runtime-release.env').read_text().splitlines())
    sha = release['RELEASE_SHA']
    if unit['fence'] and not start_is_allowed(workspace / 'control', 'production', sha,
                                             unit['database'], owner_uid=os.getuid()):
        sys.exit(21)
    spec = importlib.util.spec_from_file_location('guard', root / 'scripts/verify_deploy_start_guard.py')
    guard = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(guard)
    if not guard._start_is_allowed('production', sha):
        sys.exit(22)
    result = subprocess.run([sys.executable, str(root / 'runtime.py'), str(workspace)])
    sys.exit(result.returncode)
except (OSError, KeyError, ValueError):
    sys.exit(23)
"""

RUNTIME = """
import json, pathlib, sqlite3, sys
workspace = pathlib.Path(sys.argv[1])
root = workspace / 'be'
actual = (root / '.env').read_text().strip()
release = dict(line.split('=', 1) for line in (root / '.runtime-release.env').read_text().splitlines())
monitor = dict(line.split('=', 1) for line in (root / '.runtime-monitor.env').read_text().splitlines())
venv = (root / 'venv/receipt').read_text()
with sqlite3.connect(workspace / (actual + '.sqlite')) as connection:
    data_identity = connection.execute('SELECT identity FROM data').fetchone()[0]
record = dict(code=EXPECTED_DB, database=data_identity, release=release['RELEASE_SHA'],
              monitor=monitor['RELEASE_SHA'], venv=venv, expected_sha=EXPECTED_SHA)
with (workspace / 'executions').open('a') as output:
    output.write(json.dumps(record) + '\\n')
if (actual != EXPECTED_DB or data_identity != EXPECTED_DB or
        release['RELEASE_SHA'] != EXPECTED_SHA or monitor['RELEASE_SHA'] != EXPECTED_SHA or venv != EXPECTED_SHA):
    sys.exit(24)
print(json.dumps(dict(status='ok', db='ok', environment='production', release_sha=EXPECTED_SHA)))
"""


class TupleFixture:
    def __init__(self, workspace):
        self.workspace = workspace
        self.root = workspace / 'be'
        self.control = workspace / 'control'
        self.control.mkdir(mode=0o700)
        (self.control / 'fence').write_text('phase0\n')
        (self.control / 'fence').chmod(0o600)
        self.process = None
        (workspace / 'maintenance').write_text('closed')
        for database, sha, directory in [('verdaxis', OLD_SHA, self.root),
                                         ('verdaxis_prod', NEW_SHA, workspace / 'candidate')]:
            with sqlite3.connect(workspace / f'{database}.sqlite') as connection:
                connection.execute('CREATE TABLE data (identity TEXT)')
                connection.execute('INSERT INTO data VALUES (?)', (database,))
                connection.execute('CREATE TABLE customer_writes (value TEXT)')
            (directory / 'scripts').mkdir(parents=True)
            (directory / 'venv').mkdir()
            (directory / '.runtime-deploy').mkdir(mode=0o700)
            shutil.copyfile(ROOT / 'scripts/verify_deploy_start_guard.py', directory / 'scripts/verify_deploy_start_guard.py')
            (directory / 'runtime.py').write_text(f'EXPECTED_DB={database!r}\nEXPECTED_SHA={sha!r}\n' + RUNTIME)
            (directory / '.env').write_text(database)
            (directory / '.runtime-release.env').write_text(self.identity(sha))
            (directory / '.runtime-monitor.env').write_text(self.identity(sha))
            (directory / 'venv/receipt').write_text(sha)
        # Deliberately retain old credentials/identity in the prepared candidate.
        # Separate publications below must be safe even in this mixed state.
        candidate = workspace / 'candidate'
        (candidate / '.env').write_text('verdaxis')
        for name in ('.runtime-release.env', '.runtime-monitor.env'):
            (candidate / name).write_text(self.identity(OLD_SHA))
        self.unit = dict(fence=True, database='verdaxis_prod', release=OLD_SHA)
        self.replace(workspace / 'unit', json.dumps(self.unit))
        self.reload_unit()
        self.process = subprocess.Popen([sys.executable, '-c', '''
import fcntl, os, pathlib, sys
path = pathlib.Path(sys.argv[1]) / 'operator.lock'
with path.open('w+') as handle:
    os.chmod(path, 0o600)
    fcntl.flock(handle, fcntl.LOCK_EX)
    print('locked', flush=True)
    sys.stdin.read()
''', str(self.control)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True)
        assert self.process.stdout.readline().strip() == 'locked'

    @staticmethod
    def identity(sha):
        return f'ENVIRONMENT=production\nRELEASE_SHA={sha}\n'

    @staticmethod
    def replace(path, contents):
        temporary = path.with_name(path.name + '.tmp')
        temporary.write_text(contents)
        os.replace(temporary, path)

    def reload_unit(self):
        self.replace(self.workspace / 'loaded-unit', (self.workspace / 'unit').read_text())

    def write_environment(self, database):
        self.replace(self.root / '.env', database)

    def authorize(self, sha, database):
        self.interrupt()
        permit = self.control / 'permit.json'
        permit.unlink(missing_ok=True)
        # Fresh inode per operator window; never revive a stale permit.
        self.replace(self.control / 'operator.lock', '')
        self.process = subprocess.Popen(
            [sys.executable, '-c', LOCK_HOLDER, str(self.control), str(self.root), sha, database],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        assert self.process.stdout.readline().strip() == 'locked'

    def install_new_unit(self):
        self.unit['release'] = NEW_SHA
        self.replace(self.workspace / 'unit', json.dumps(self.unit))

    def advance_to(self, boundary):
        steps = [
            ('old-renamed', lambda: self.root.rename(self.workspace / 'rollback')),
            ('new-renamed', lambda: (self.workspace / 'candidate').rename(self.root)),
            ('environment', lambda: self.write_environment('verdaxis_prod')),
            ('runtime-identity', lambda: self.replace(self.root / '.runtime-release.env', self.identity(NEW_SHA))),
            ('monitor-identity', lambda: self.replace(self.root / '.runtime-monitor.env', self.identity(NEW_SHA))),
            ('unit-written', self.install_new_unit),
            ('unit-loaded', self.reload_unit),
            ('authorized', lambda: self.authorize(NEW_SHA, 'verdaxis_prod')),
            ('restart', lambda: self.require_ready(NEW_SHA)),
            ('readiness', lambda: self.require_ready(NEW_SHA)),
        ]
        for name, operation in steps:
            operation()
            self.assert_public_write_blocked()
            if name == boundary:
                return
        raise AssertionError('Unknown boundary')

    def start(self):
        return subprocess.run([sys.executable, '-c', START_ADAPTER, str(self.workspace), str(ROOT)],
                              capture_output=True, text=True, timeout=10)

    def require_ready(self, sha):
        result = self.start()
        assert result.returncode == 0, result.stderr
        assert json.loads(result.stdout) == dict(status='ok', db='ok', environment='production', release_sha=sha)

    def assert_starts_match_or_are_blocked(self):
        self.start()
        self.assert_no_mismatched_execution()

    def assert_public_write_blocked(self):
        # A separate executable routing boundary, independent of operator locks.
        result = subprocess.run([sys.executable, '-c', '''
import pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1])
if (root / 'maintenance').read_text() == 'closed':
    sys.exit(43)
with sqlite3.connect(root / 'verdaxis_prod.sqlite') as connection:
    connection.execute("INSERT INTO customer_writes VALUES ('request')")
''', str(self.workspace)], capture_output=True, timeout=10)
        assert result.returncode == 43
        for database in ('verdaxis', 'verdaxis_prod'):
            with sqlite3.connect(self.workspace / f'{database}.sqlite') as connection:
                assert connection.execute('SELECT count(*) FROM customer_writes').fetchone()[0] == 0

    def assert_no_mismatched_execution(self):
        log = self.workspace / 'executions'
        for line in log.read_text().splitlines() if log.exists() else []:
            record = json.loads(line)
            assert record['code'] == record['database']
            assert record['release'] == record['monitor'] == record['venv'] == record['expected_sha']

    def interrupt(self):
        if self.process is not None:
            self.process.kill()
            self.process.wait(timeout=5)
            for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
                stream.close()
            self.process = None

    def rollback(self):
        self.interrupt()
        if self.root.exists():
            self.root.rename(self.workspace / 'failed-new')
        (self.workspace / 'rollback').rename(self.root)
        self.unit['database'] = 'verdaxis'
        self.unit['release'] = OLD_SHA
        self.replace(self.workspace / 'unit', json.dumps(self.unit))
        self.authorize(OLD_SHA, 'verdaxis')
        # The old permit cannot pass the cached new database literal.
        assert self.start().returncode != 0
        self.reload_unit()
        self.require_ready(OLD_SHA)

    def restore_source_only(self):
        if not self.root.exists():
            (self.workspace / 'rollback').rename(self.root)

    def disable_loaded_fence(self):
        self.unit['fence'] = False
        self.replace(self.workspace / 'unit', json.dumps(self.unit))
        self.reload_unit()

    def close(self):
        self.interrupt()
