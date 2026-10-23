#!/usr/bin/env python3
"""Fixed read-only host reader, sent on SSH stdin; standard library only.

Never imports inspected code, reads process arguments/environment, or writes files.
Missing permissions and bounds produce incomplete coverage, not a clean inventory.
"""
import ast
import hashlib
import json
import os
import re
import selectors
import signal
import stat
import subprocess
import sys
import time
from pathlib import PurePosixPath

MAX_SCRIPT_BYTES = 262144
MAX_OUTPUT_BYTES = 2097152
MAX_ITEMS = 1000
MAX_TOTAL_BYTES = 8388608

# THE REVIEWED REMOTE COMMAND ALLOWLIST. No shell execution or arbitrary argv.
# sudo is used ONLY for these exact three crontab reads. systemctl needs no sudo.
COMMANDS = {
    'units': ('systemctl', 'list-unit-files', '--no-pager', '--no-legend'),
    'timers': ('systemctl', 'list-timers', '--all', '--no-pager', '--no-legend'),
    'processes': ('ps', '-eo', 'pid=,user=,comm='),
    'executables': ('find', '/usr/local/bin', '/usr/local/sbin', '/usr/local/libexec',
                    '-xdev', '-type', 'f', '-perm', '/111', '-print'),
    'packages': ('dpkg-query', '-W', '-f=${binary:Package}\t${Version}\n'),
    'cron-root': ('sudo', '-n', 'crontab', '-l', '-u', 'root'),
    'cron-service': ('sudo', '-n', 'crontab', '-l', '-u', 'verdaxis-prod'),
    'cron-operator': ('sudo', '-n', 'crontab', '-l', '-u', 'jons-openclaw'),
}
UNIT_NAME = re.compile(r'[A-Za-z0-9_@.:-]+\.(?:service|timer)')
SECRET_KEY = re.compile(r'(?i)(PASS|SECRET|TOKEN|KEY|DSN|DATABASE_URL|AUTH|CREDENTIAL|COOKIE)')
ASSIGNMENT = re.compile(r'\b([A-Za-z_][A-Za-z0-9_]*)=(<redacted>|"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)*\'|[^\s;]+)')


def redact(text):
    # Consume continued lines first: no second line of a secret may survive.
    text = re.sub(r'\\\r?\n', ' ', text)
    text = re.sub(r'(?im)\b(Environment(?:File)?\s*)=.*$', r'\1=<redacted>', text)
    text = ASSIGNMENT.sub(lambda match: match[1] + '=<redacted>'
                          if SECRET_KEY.search(match[1]) else match[0], text)
    text = re.sub(r'[A-Za-z][A-Za-z0-9+.-]*://[^\s/]+@[^\s]*', '<redacted>', text)
    # Hex is a subset of base64. Redact long runs even without word boundaries.
    # Assignment keys must remain visible; redact only the runs in their values.
    chunks = re.split(r'(\b[A-Za-z_][A-Za-z0-9_]*=)', text)
    return ''.join(chunk if index % 2 else re.sub(r'[A-Za-z0-9+/_=-]{32,}', '<redacted>', chunk)
                   for index, chunk in enumerate(chunks))


def forbidden(path):
    parts = PurePosixPath(path).parts
    return (not path.startswith('/') or '..' in parts or
            any(part.lower() == 'artifact-provenance' or part.lower().startswith('.env') or
                re.search(r'(?i)(credential|secret|password|passwd|token|(^|[._-])keys?($|[._-])|^id_rsa|^id_ed25519|\.(pem|p12|pfx)$)', part)
                for part in parts))


def read_file(path):
    if forbidden(path):
        raise ValueError('credential path forbidden')
    directory = os.open('/', os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in PurePosixPath(path).parts[1:-1]:
            next_directory = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                     dir_fd=directory)
            os.close(directory)
            directory = next_directory
        descriptor = os.open(PurePosixPath(path).name,
                             os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory)
        with os.fdopen(descriptor, 'rb') as stream:
            before = os.fstat(stream.fileno())
            if not stat.S_ISREG(before.st_mode) or before.st_size > MAX_SCRIPT_BYTES:
                raise ValueError('script size or type refused')
            raw = stream.read(MAX_SCRIPT_BYTES + 1)
            after = os.fstat(stream.fileno())
            if len(raw) > MAX_SCRIPT_BYTES or (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns):
                raise ValueError('script changed or exceeded bound')
    finally:
        os.close(directory)
    return raw, before


def read_script(path):
    raw, before = read_file(path)
    text = raw.decode('utf-8', errors='replace')
    imports = set()
    references = set()
    imports_complete = True
    # Unknown executable formats are metadata-only and cannot prove import safety.
    if b'\x00' in raw or not (path.endswith(('.py', '.sh')) or text.startswith('#!')):
        imports_complete = False
    if path.endswith('.py'):
        try:
            tree = ast.parse(text)
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    imports.update(alias.name for alias in node.names)
                elif isinstance(node, ast.ImportFrom):
                    imports.add('.' * node.level + (node.module or ''))
                elif isinstance(node, ast.Call) and (isinstance(node.func, ast.Name) and node.func.id in {'__import__', 'eval', 'exec'} or isinstance(node.func, ast.Attribute) and node.func.attr == 'import_module'):
                    imports_complete = False
        except (SyntaxError, RecursionError):
            imports_complete = False
    else:
        for match in re.finditer(r'(?:^|[;\n])\s*(?:\.|source)\s+["\']?([^\s"\';]+)', text):
            name = match[1]
            if forbidden(name):
                references.add(name)
            else:
                imports.add(name)
        if '$' in text or '`' in text:
            imports_complete = False  # static discovery cannot prove dynamic references
    # File contents are never returned. Only static imports and metadata survive.
    return {'path': redact(path), 'uid': before.st_uid, 'gid': before.st_gid,
            'mode': format(stat.S_IMODE(before.st_mode), '04o'), 'size': len(raw),
            'sha256': hashlib.sha256(raw).hexdigest(), 'imports': sorted(redact(name) for name in imports),
            'imports_complete': imports_complete, 'credential_references': sorted(redact(name) for name in references)}


def allowed(argv):
    argv = tuple(argv)
    return argv in COMMANDS.values() or (len(argv) == 5 and
        argv[:4] == ('systemctl', 'cat', '--no-pager', '--') and
        UNIT_NAME.fullmatch(argv[4]) is not None)


def run_command(argv):
    if not allowed(argv):
        raise ValueError('command not allowlisted')
    # Drain both streams with byte and wall-clock limits. Never return stderr.
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True,
                               env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL': 'C',
                                    'SYSTEMD_PAGER': 'cat', 'SYSTEMD_COLORS': '0'})
    data = bytearray()
    total = 0
    deadline = time.monotonic() + 15
    complete = True
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            selector.register(process.stderr, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() >= deadline:
                    complete = False
                    break
                for key, _ in selector.select(timeout=0.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    total += len(chunk)
                    if total > MAX_OUTPUT_BYTES:
                        complete = False
                        break
                    if key.fileobj is process.stdout:
                        data.extend(chunk)
                if not complete:
                    break
        if not complete:
            os.killpg(process.pid, signal.SIGKILL)
        try:
            status = process.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            status = process.wait()
            complete = False
        return (data.decode('utf-8', errors='replace') if complete else ''), complete and status == 0
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        process.stdout.close()
        process.stderr.close()


def collect(command=run_command, cron_files=None, script_reader=read_script):
    observations = []
    coverage = {}
    scripts = []
    paths = set()
    budget = 0

    def observe(surface, argv):
        nonlocal budget
        text, complete = command(argv)
        safe = redact(text)  # before storage or transport
        budget += len(safe.encode())
        if budget > MAX_TOTAL_BYTES:
            raise ValueError('inventory output bound exceeded')
        observations.append({'surface': surface, 'text': safe, 'complete': complete})
        coverage[surface] = coverage.get(surface, True) and complete
        for path in re.findall(r'/[A-Za-z0-9_./-]+\.(?:sh|py)\b', safe):
            if not forbidden(path):
                paths.add(path)
        return safe

    units = observe('units', COMMANDS['units'])
    names = [line.split()[0] for line in units.splitlines() if line.split() and UNIT_NAME.fullmatch(line.split()[0])]
    if len(names) > MAX_ITEMS:
        raise ValueError('unit count bound exceeded')
    coverage['dropins'] = True
    for unit in sorted(set(names)):
        observe('dropins', ('systemctl', 'cat', '--no-pager', '--', unit))
    observe('timers', COMMANDS['timers'])
    observe('processes', COMMANDS['processes'])
    executables = observe('usr_local', COMMANDS['executables'])
    for path in executables.splitlines():
        if re.search(r'(?i)(backup|restore|pull|dump|fleet)', path) and not forbidden(path):
            paths.add(path)
    for name in ('cron-root', 'cron-service', 'cron-operator'):
        observe('cron', COMMANDS[name])
    # System cron files are read as bounded regular files, never sourced.
    coverage['cron'] = coverage.get('cron', True)
    cron_paths = list(cron_files) if cron_files is not None else ['/etc/crontab']
    try:
        if cron_files is None:
            descriptor = os.open('/etc/cron.d', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                with os.scandir(descriptor) as entries:
                    for entry in entries:
                        if len(cron_paths) >= MAX_ITEMS:
                            raise ValueError('cron count bound exceeded')
                        if entry.is_file(follow_symlinks=False):
                            cron_paths.append('/etc/cron.d/' + entry.name)
            finally:
                os.close(descriptor)
        for path in cron_paths:
            raw, _ = read_file(path)
            safe = redact(raw.decode('utf-8', errors='replace'))
            budget += len(safe.encode())
            if budget > MAX_TOTAL_BYTES:
                raise ValueError('inventory output bound exceeded')
            observations.append({'surface': 'cron', 'text': safe, 'complete': True})
            for name in re.findall(r'/[A-Za-z0-9_./-]+\.(?:sh|py)\b', safe):
                if not forbidden(name):
                    paths.add(name)
    except (OSError, ValueError):
        coverage['cron'] = False
    observe('packages', COMMANDS['packages'])
    coverage['imports'] = True
    visited = set()
    while paths:
        path = min(paths)
        paths.remove(path)
        if path in visited:
            continue
        if len(visited) >= MAX_ITEMS:
            raise ValueError('script count bound exceeded')
        visited.add(path)
        try:
            record = script_reader(path)
            scripts.append(record)
            coverage['imports'] &= record['imports_complete']
            for name in record['imports']:
                if name.startswith('/usr/local/') and not forbidden(name):
                    paths.add(name)
        except (OSError, ValueError):
            coverage['imports'] = False
            observations.append({'surface': 'imports', 'text': redact(path) + ': unreadable or refused', 'complete': False})
    return {'coverage': coverage, 'observations': observations, 'scripts': scripts}


if __name__ == '__main__':
    try:
        result = collect()
        output = json.dumps(result, ensure_ascii=True)
        if len(output.encode()) > MAX_TOTAL_BYTES:
            raise ValueError('output too large')
        sys.stdout.write(output + '\n')
    except (OSError, ValueError, RecursionError):
        sys.stderr.write('read-only inventory refused\n')
        sys.exit(1)
