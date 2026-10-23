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
import shlex
import signal
import stat
import subprocess
import sys
import time
from pathlib import PurePosixPath

MAX_SCRIPT_BYTES = 262144
MAX_OUTPUT_BYTES = 2097152
MAX_ITEMS = 1000
MAX_TOTAL_BYTES = 7340032  # leave transport framing headroom below 8 MiB

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


REFUSAL_CODES = {'output-bytes', 'os-error', 'value-error', 'recursion'}
BOUND_CODES = {'unit-count', 'structural-items', 'blocking-rows', 'cron-count',
               'entries', 'import-queue', 'statements', 'logical-lines',
               'graph-bytes', 'references', 'command-bytes'}


class SurfaceBound(ValueError):
    def __init__(self, code):
        self.code = code


def row_priority(row):
    status = row.get('status', '')
    if status == 'credential-denied':
        return 1
    if status == 'removed-package':
        return 2
    if status.startswith(('bound-exceeded:', 'overflow:')):
        return 3
    if status:
        return 4
    return 5


class PriorityRows:
    """Bound each evidence class; reserved marker slots are outside data budgets.

    Later high-priority evidence cannot be crowded out by ordinary rows. Only
    counts survive for dropped rows, and class overflow markers are indelible.
    """
    def __init__(self, capacity):
        self.capacity = capacity
        self.rows = {}
        self.dropped = {}
        self.markers = {}

    def add(self, row):
        priority = row_priority(row)
        if priority == 3:
            self.markers[row['status']] = row
            return
        key = json.dumps(row, sort_keys=True)
        bucket = self.rows.setdefault(priority, {})
        bucket[key] = row
        if len(bucket) > self.capacity:
            del bucket[max(bucket)]
            self.dropped[str(priority)] = self.dropped.get(str(priority), 0) + 1
            if priority in (1, 2):
                name = 'credential-denied' if priority == 1 else 'removed-package'
                self.markers[name] = dict(bound_row(name), status='overflow:' + name)

    def values(self):
        markers = list(self.markers.values())
        return sorted(markers + [row for bucket in self.rows.values() for row in bucket.values()],
                      key=lambda row: (row_priority(row), json.dumps(row, sort_keys=True)))


def refusal(error):
    if isinstance(error, SurfaceBound) and error.code == 'output-bytes':
        return {'refusal': 'output-bytes'}
    if isinstance(error, RecursionError):
        return {'refusal': 'recursion'}
    if isinstance(error, OSError):
        return {'refusal': 'os-error'}
    return {'refusal': 'value-error'}


def bound_row(surface):
    return {'source': unresolved('<bound>'), 'reference': unresolved('<bound>'),
            'status': 'bound-exceeded:' + surface}


def encode_result(result):
    encoder = json.JSONEncoder(ensure_ascii=True)
    size = sum(len(chunk.encode()) for chunk in encoder.iterencode(result))
    if size > MAX_TOTAL_BYTES:
        return json.dumps({'refusal': 'output-bytes'}) + '\n'
    return ''.join(encoder.iterencode(result)) + '\n'


def redact(text):
    # Consume continued lines first: no second line of a secret may survive.
    text = re.sub(r'\\\r?\n', ' ', text)
    text = re.sub(r'(?im)\b(Environment(?:File)?\s*)=.*$', r'\1=<redacted>', text)
    # Ambiguous shell quoting/escapes and non-assignment credential transports
    # cannot be safely reconstructed. Keep the credential key/flag, drop its tail.
    text = re.sub(r'(?im)(--[A-Za-z_-]*(?:pass|secret|token|key|auth|credential|cookie)[A-Za-z_-]*)(?:\s+|=).*$' , r'\1=<redacted>', text)
    text = re.sub(r'(?im)\b(Authorization|Cookie)\s*:.*$', r'\1=<redacted>', text)
    lines = []
    for line in text.splitlines(keepends=True):
        if '\\' in line or line.count('"') % 2 or line.count("'") % 2:
            match = re.search(r'\b([A-Za-z_][A-Za-z0-9_]*)=', line)
            if match and SECRET_KEY.search(match[1]):
                line = line[:match.start()] + match[1] + '=<redacted>' + ('\n' if line.endswith('\n') else '')
        lines.append(line)
    text = ''.join(lines)
    text = ASSIGNMENT.sub(lambda match: match[1] + '=<redacted>'
                          if SECRET_KEY.search(match[1]) else match[0], text)
    text = re.sub(r'[A-Za-z][A-Za-z0-9+.-]*://[^\s/]+@[^\s]*', '<redacted>', text)
    # Hex is a subset of base64. Redact long runs even without word boundaries.
    # Assignment keys must remain visible; redact only the runs in their values.
    chunks = re.split(r'(\b[A-Za-z_][A-Za-z0-9_]{0,30}=)', text)
    return ''.join(chunk if index % 2 else re.sub(r'[A-Za-z0-9+/_=-]{32,}', '<redacted>', chunk)
                   for index, chunk in enumerate(chunks))


# Exact inspection roots; fixture tests replace this tuple, never extend live CLI.
FILE_ROOTS = ('/etc/systemd/system', '/usr/lib/systemd/system', '/lib/systemd/system',
              '/etc/cron.d', '/usr/local/bin', '/usr/local/sbin', '/usr/local/libexec')
FILE_EXACT = ('/etc/crontab',)


def file_allowed(path):
    if forbidden(path):
        return False
    return path in FILE_EXACT or any(path.startswith(root + '/') for root in FILE_ROOTS)


def forbidden(path):
    parts = PurePosixPath(path).parts
    return (not path.startswith('/') or '..' in parts or
            any(part.lower() in {'.pgpass', 'shadow', 'gshadow', '.ssh', '.netrc', '.git-credentials', 'verdaxis'} or part.lower() == 'artifact-provenance' or part.lower().startswith('.env') or part.lower().endswith('.env') or
                re.search(r'(?i)(credential|secret|password|passwd|token|(^|[._-])keys?($|[._-])|^id_rsa|^id_ed25519|\.(pem|p12|pfx)$)', part)
                for part in parts))


def read_file(path):
    if forbidden(path):
        raise ValueError('credential path forbidden')
    if not file_allowed(path):
        raise ValueError('path outside inspection allowlist')
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


def scan_script(path):
    raw, before = read_file(path)
    text = raw.decode('utf-8', errors='replace')
    imports = set()
    references = set()
    scan_parsed = True
    executables = set()
    # Unknown executable formats are metadata-only and cannot prove import safety.
    if b'\x00' in raw or not (path.endswith(('.py', '.sh')) or text.startswith('#!')):
        scan_parsed = False
    if path.endswith('.py'):
        try:
            tree = ast.parse(text)
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    imports.update(alias.name for alias in node.names)
                elif isinstance(node, ast.ImportFrom):
                    prefix = '.' * node.level + (node.module or '')
                    imports.add(prefix)
                    for alias in node.names:
                        if alias.name != '*':
                            imports.add(prefix + ('.' if node.module else '') + alias.name)
                elif isinstance(node, ast.Call) and (isinstance(node.func, ast.Name) and node.func.id in {'__import__', 'eval', 'exec'} or isinstance(node.func, ast.Attribute) and node.func.attr == 'import_module'):
                    scan_parsed = False
        except (SyntaxError, RecursionError):
            scan_parsed = False
    else:
        for statement in logical_statements(text):
            try:
                words = shlex.split(statement)
            except ValueError:
                imports.add(statement)
                scan_parsed = False
                continue
            if not words or words[0].startswith('#'):
                continue
            if words[0] in {'.', 'source'}:
                if len(words) == 2 and safe_path(words[1]):
                    if forbidden(words[1]):
                        references.add(words[1])
                    else:
                        imports.add(words[1])
                else:
                    imports.add(statement)
                    scan_parsed = False
            else:
                operand, certain = script_operand(statement)
                if operand:
                    executables.add(operand)
                if not certain:
                    imports.add(statement)
                    scan_parsed = False
    if not scan_parsed:
        imports.add('<dynamic-or-unparsed>')
    truncated = []
    dropped = {}
    # Reserve denial and removed-package capacity before ordinary references.
    groups = [(1, 'credential_references', sorted(references)),
              (2, 'imports', sorted(name for name in imports if name.split('.')[0] in {'app', 'scripts', 'alembic'})),
              (4, 'imports', sorted(name for name in imports if name.split('.')[0] not in {'app', 'scripts', 'alembic'})),
              (5, 'executables', sorted(executables))]
    retained = {'credential_references': [], 'imports': [], 'executables': []}
    overflow = []
    for priority, field, values in groups:
        retained[field].extend(values[:MAX_ITEMS])
        if len(values) > MAX_ITEMS:
            truncated.append('references')
            dropped[str(priority)] = len(values) - MAX_ITEMS
            if priority in (1, 2):
                overflow.append('credential-denied' if priority == 1 else 'removed-package')
    imports = retained['imports']
    references = retained['credential_references']
    executables = retained['executables']
    # File contents are never returned. Only static imports and metadata survive.
    return {'path': path if safe_path(path) else '<unparsed>', 'uid': before.st_uid, 'gid': before.st_gid,
            'mode': format(stat.S_IMODE(before.st_mode), '04o'), 'size': len(raw),
            'sha256': hashlib.sha256(raw).hexdigest(), 'imports': sorted(imports),
            'read_acquired': True, 'credential_references': sorted(references),
            'executables': sorted(executables), 'truncations': sorted(set(truncated)),
            'dropped': dropped, 'priority_overflow': overflow}


# Only these structural values may cross the host boundary (PD10-W1).
PATH_VALUE = re.compile(r'/[A-Za-z0-9_./@+-]{1,4095}')
NAME_VALUE = re.compile(r'[A-Za-z_][A-Za-z0-9_.@:-]{0,127}')
TIMER_KEYS = {'OnCalendar', 'OnBootSec', 'OnStartupSec', 'OnActiveSec',
              'OnUnitActiveSec', 'OnUnitInactiveSec', 'AccuracySec', 'RandomizedDelaySec'}
APP_ROOTS = ('/home/verdaxis-prod/verdaxis/prod/be', '/home/verdaxis-prod/verdaxis/staging/be')


def sha(text):
    return hashlib.sha256(text.encode()).hexdigest()


def safe_path(value):
    return PATH_VALUE.fullmatch(value) is not None and '..' not in PurePosixPath(value).parts


def logical_statements(text):
    pending = ''
    for line in text.splitlines(keepends=True):
        pending += line
        if len(pending.encode()) > MAX_SCRIPT_BYTES:
            raise SurfaceBound('statements')
        if line.rstrip('\r\n').endswith('\\'):
            continue
        try:
            shlex.split(pending.replace('\\\n', ' '))
        except ValueError:
            continue
        yield pending.replace('\\\n', ' ').strip()
        pending = ''
    if pending:
        yield pending


def script_operand(value):
    """PD10-W2: one script, optional exact interpreter, no executable tail."""
    if any(char in value for char in ';|&<>`$*?[]{}\n\r\\'):
        return None, False
    try:
        words = shlex.split(value)
    except ValueError:
        return None, False
    prefixes = ((), ('python3',), ('/usr/bin/python3',),
                ('/usr/bin/env', 'python3'), ('/bin/sh',), ('bash',))
    for prefix in prefixes:
        if len(words) == len(prefix) + 1 and tuple(words[:-1]) == prefix:
            path = words[-1]
            if safe_path(path) and not forbidden(path):
                return path, True
    return None, False


def entry_operands(surface, text, system_cron=False):
    for line in logical_statements(text):
        invocation = None
        if surface == 'dropins' and '=' in line:
            key, value = line.split('=', 1)
            if key.strip().startswith('Exec'):
                invocation = value.strip()
            elif key.strip() == 'EnvironmentFile':
                yield value, (None, False)
        elif surface == 'cron' and line and not line.startswith('#'):
            parts = line.split(None, 6 if system_cron else 5)
            count = 6 if system_cron else 5
            if len(parts) == count + 1 and all(re.fullmatch(r'[0-9*/,-]+', item) for item in parts[:5]):
                invocation = parts[count]
            elif line.startswith('@'):
                parts = line.split(None, 2 if system_cron else 1)
                invocation = parts[-1] if len(parts) == (3 if system_cron else 2) else None
        if invocation is not None:
            yield invocation, script_operand(invocation)
        elif surface == 'cron' and line and not line.startswith('#'):
            yield line, (None, False)


def executable(value):
    # shlex is a tokenizer, never a shell. Only argv[0] survives; assignments,
    # wrappers and all arguments remain host-local. Ambiguous syntax is refused.
    try:
        words = shlex.split(value)
    except ValueError:
        return None
    if not words:
        return None
    first = words[0].lstrip('-+!:@')
    return first if safe_path(first) else None


def structural(surface, text, system_cron=False):
    fields = []
    unparsed = []
    # A physical multiline quoted value is uncertain: suppress the entire
    # continuation until its quote closes, rather than parsing suffixes as fields.
    truncated = []
    pending = ''
    for physical in text.splitlines(keepends=True):
        pending += physical
        if len(pending.encode()) > MAX_OUTPUT_BYTES:
            truncated.append('logical-lines')
            pending = ''
            break
        if physical.rstrip('\r\n').endswith('\\'):
            continue
        try:
            shlex.split(pending.replace('\\\n', ' '))
        except ValueError:
            continue
        raw = pending
        pending = ''
        line = raw.replace('\\\n', ' ').strip()
        parsed = []
        if surface == 'dropins' and '=' in line:
            key, value = line.split('=', 1)
            key, value = key.strip(), value.strip()
            if key in {'User', 'Group'} and NAME_VALUE.fullmatch(value) or key == 'WorkingDirectory' and safe_path(value):
                parsed = [value]
            elif key in {'Wants', 'Requires', 'After'} and value and all(UNIT_NAME.fullmatch(v) or re.fullmatch(r'[A-Za-z0-9_.@:-]+\.(?:target|socket|mount|path)', v) for v in value.split()):
                parsed = value.split()
            elif key in TIMER_KEYS and value and re.fullmatch(r'[A-Za-z0-9*,:./+~ _=-]{1,256}', value):
                parsed = [value]
            elif key in {'ExecStart', 'ExecStartPre', 'ExecStop'}:
                path = executable(value)
                if path:
                    parsed = [path]
            elif key == 'Environment':
                words = shlex.split(value)
                if words and all('=' in w and re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,127}', w.split('=', 1)[0]) for w in words):
                    parsed = sorted({w.split('=', 1)[0] for w in words})
            elif key == 'EnvironmentFile':
                words = shlex.split(value)
                if words and all(safe_path(w.lstrip('-')) for w in words):
                    parsed = [w.lstrip('-') for w in words]
            if parsed:
                fields.append({'key': key, 'values': [redact(v) for v in parsed]})
        elif surface == 'cron' and line and not line.startswith('#'):
            parts = line.split(None, 6 if system_cron else 5)
            count = 6 if system_cron else 5
            schedule = parts[:5]
            if len(parts) == count + 1 and all(re.fullmatch(r'[0-9*/,-]+', v) for v in schedule):
                path = executable(parts[count])
                if path:
                    fields.extend([{'key': 'schedule', 'values': [' '.join(schedule)]},
                                   {'key': 'executable', 'values': [redact(path)]}])
                    parsed = [path]
            elif line.startswith('@'):
                parts = line.split(None, 2 if system_cron else 1)
                if len(parts) == (3 if system_cron else 2) and parts[0] in {'@reboot', '@hourly', '@daily', '@weekly', '@monthly', '@yearly', '@annually', '@midnight'}:
                    path = executable(parts[-1])
                    if path:
                        fields.extend([{'key': 'schedule', 'values': [parts[0]]}, {'key': 'executable', 'values': [redact(path)]}])
                        parsed = [path]
        elif surface in {'units', 'timers'}:
            names = [word for word in line.split() if UNIT_NAME.fullmatch(word)]
            if names:
                fields.append({'key': 'unit', 'values': [redact(n) for n in names]})
                parsed = names
        elif surface == 'usr_local' and safe_path(line):
            fields.append({'key': 'executable', 'values': [redact(line)]})
            parsed = [line]
        if not parsed:
            unparsed.append({'value': '<unparsed>', 'sha256': sha(raw)})
        if len(fields) + len(unparsed) > MAX_ITEMS:
            fields = fields[:MAX_ITEMS]
            unparsed = unparsed[:max(0, MAX_ITEMS - len(fields))]
            truncated.append('structural-items')
            break
    if pending:
        unparsed.append({'value': '<unparsed>', 'sha256': sha(pending)})
    return {'surface': surface, 'sha256': sha(text), 'fields': fields, 'unparsed': unparsed, 'truncations': truncated}


def unresolved(value):
    return {'kind': '<unresolved>', 'sha256': sha(value)}


def resolved(path):
    return {'kind': 'resolved', 'path': path}


def inspect_graph(paths, roots=APP_ROOTS, max_depth=16, script_reader=scan_script):
    scripts = []
    blocking = PriorityRows(MAX_ITEMS)
    visited = set()
    inspected = set()
    queue = [(path, 0) for path in sorted(set(paths))]
    budget = 0
    truncated = set()

    def block(source, reference, status='unresolved'):
        blocking.add({'source': resolved(source) if source in inspected else unresolved(source),
                         'reference': unresolved(reference), 'status': status})


    while queue:
        path, depth = queue.pop(0)
        if path in visited:
            continue
        if len(visited) >= MAX_ITEMS or depth > max_depth:
            truncated.add('import-queue')
            break
        visited.add(path)
        try:
            record = script_reader(path)
        except SurfaceBound as error:
            truncated.add(error.code)
            continue
        except (OSError, ValueError):
            block(path, '<unreadable-or-forbidden>')
            continue
        budget += len(json.dumps(record).encode())
        if budget > MAX_TOTAL_BYTES:
            truncated.add('graph-bytes')
            break
        inspected.add(path)
        truncated.update(record.get('truncations', []))
        for name in record.get('priority_overflow', []):
            blocking.add(dict(bound_row(name), status='overflow:' + name))
        public = dict(record, imports=[], executables=[], credential_references=[])
        public['truncations'] = record.get('truncations', [])
        scripts.append(public)
        # Scan uncertainty is represented by unresolved references, never completeness.
        for credential in record['credential_references']:
            # Credential files must never be inspected, so they cannot acquire
            # structural identity. Even the path remains host-local.
            public['credential_references'].append(unresolved(credential))
            block(path, credential, 'credential-denied')
        for field in ('imports', 'executables'):
            for reference in record.get(field, []):
                if reference.split('.')[0] in {'app', 'scripts', 'alembic'}:
                    block(path, reference, 'removed-package')
                # No installed invocation search context is attested here.
                # Python names (including stdlib names) therefore stay unresolved.
                # Only literal absolute source/script paths can be inspected.
                if not safe_path(reference) or forbidden(reference) or depth >= max_depth:
                    public[field].append(unresolved(reference))
                    block(path, reference)
                    continue
                try:
                    script_reader(reference)
                except (OSError, ValueError):
                    public[field].append(unresolved(reference))
                    block(path, reference)
                    continue
                public[field].append(resolved(reference))
                queue.append((reference, depth + 1))
                if len(queue) > MAX_ITEMS:
                    queue = queue[:MAX_ITEMS]
                    truncated.add('import-queue')
    if blocking.dropped:
        truncated.add('blocking-rows')
    for code in sorted(truncated):
        blocking.add(bound_row(code))
    return {'scripts': scripts, 'blocking_dependencies': blocking.values(), 'dropped': blocking.dropped, 'truncations': sorted(truncated), 'dependency_coverage': 'manual-review-required'}


def read_script(path):
    # Public helper uses the same projection as the serialized remote graph.
    graph = inspect_graph([path])
    if not graph['scripts']:
        # Preserve no-follow/denial errors for callers without disclosing text.
        scan_script(path)
        raise ValueError('script unresolved')
    return graph['scripts'][0]


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
        # On truncation retain complete physical lines only. No partial token
        # can become a structural field at the byte boundary.
        if not complete:
            data = data[:data.rfind(b'\n') + 1]
        return data.decode('utf-8', errors='replace'), complete and status == 0
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        process.stdout.close()
        process.stderr.close()


def collect(command=run_command, cron_files=None, script_reader=scan_script):
    observations = []
    coverage = {}
    paths = set()
    entry_blocks = []
    truncated = {}
    budget = 0

    def mark(surface, code):
        truncated.setdefault(surface, set()).add(code)
        coverage[surface] = False

    def retain(surface, text, complete, system_cron=False):
        nonlocal budget
        entries = []
        try:
            for item in entry_operands(surface, text, system_cron):
                if len(entries) >= MAX_ITEMS:
                    mark(surface, 'entries')
                    break
                entries.append(item)
        except SurfaceBound as error:
            entries = []
            mark(surface, error.code)
        for invocation, (operand, certain) in entries[:MAX_ITEMS]:
            if operand:
                paths.add(operand)
            if not certain and len(entry_blocks) < MAX_ITEMS:
                entry_blocks.append({'source': unresolved(invocation), 'reference': unresolved(invocation), 'status': 'unresolved'})
            if len(entry_blocks) >= MAX_ITEMS:
                mark(surface, 'entries')
        if len(entries) > MAX_ITEMS:
            mark(surface, 'entries')
        record = structural(surface, text, system_cron)
        for code in record['truncations']:
            mark(surface, code)
        record['acquired'] = complete and surface not in truncated
        budget += len(json.dumps(record).encode())
        if budget > MAX_TOTAL_BYTES:
            raise SurfaceBound('output-bytes')
        observations.append(record)
        coverage[surface] = coverage.get(surface, True) and complete and surface not in truncated
        for field in record['fields']:
            if field['key'] in {'executable', 'ExecStart', 'ExecStartPre', 'ExecStop'}:
                for path in field['values']:
                    if safe_path(path) and not forbidden(path) and (path.endswith(('.sh', '.py')) or path.startswith('/usr/local/')):
                        paths.add(path)
        return record

    def observe(surface, argv):
        try:
            text, complete = command(argv)
        except OSError:
            text, complete = '', False
        if not complete:
            mark(surface, 'command-bytes')
        if surface in {'units', 'timers', 'usr_local', 'processes', 'packages'}:
            text = '\n'.join(sorted(text.splitlines()))
        return retain(surface, text, complete)

    units = observe('units', COMMANDS['units'])
    names = {name for field in units['fields'] for name in field['values'] if UNIT_NAME.fullmatch(name)}
    if len(names) > MAX_ITEMS:
        mark('units', 'unit-count')
        names = sorted(names)[:MAX_ITEMS]
    coverage['dropins'] = coverage.get('units', False)
    for unit in sorted(names):
        observe('dropins', ('systemctl', 'cat', '--no-pager', '--', unit))
    for surface, name in [('timers', 'timers'), ('processes', 'processes'),
                          ('usr_local', 'executables'), ('packages', 'packages')]:
        observe(surface, COMMANDS[name])
    for name in ('cron-root', 'cron-service', 'cron-operator'):
        observe('cron', COMMANDS[name])
    cron_paths = list(cron_files) if cron_files is not None else ['/etc/crontab']
    try:
        if cron_files is None:
            descriptor = os.open('/etc/cron.d', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
            try:
                with os.scandir(descriptor) as entries:
                    for entry in sorted(entries, key=lambda item: item.name):
                        if len(cron_paths) >= MAX_ITEMS:
                            mark('cron', 'cron-count')
                            break
                        if entry.is_file(follow_symlinks=False):
                            cron_paths.append('/etc/cron.d/' + entry.name)
            finally:
                os.close(descriptor)
        if len(cron_paths) > MAX_ITEMS:
            mark('cron', 'cron-count')
        for path in sorted(cron_paths)[:MAX_ITEMS]:
            try:
                raw, _ = read_file(path)
                retain('cron', raw.decode('utf-8', errors='strict'), True, system_cron=True)
            except SurfaceBound as error:
                if error.code == 'output-bytes':
                    raise
                mark('cron', error.code)
            except (OSError, ValueError):
                coverage['cron'] = False
                entry_blocks.append({'source': unresolved(path), 'reference': unresolved('<unreadable>'), 'status': 'unresolved'})
    except SurfaceBound as error:
        if error.code == 'output-bytes':
            raise
        mark('cron', error.code)
    except (OSError, ValueError):
        coverage['cron'] = False
    graph = inspect_graph(paths, script_reader=script_reader)
    final_rows = PriorityRows(MAX_ITEMS)
    for row in graph['blocking_dependencies'] + entry_blocks:
        final_rows.add(row)
    for code in graph['truncations']:
        mark('usr_local', code)
    for surface in sorted(truncated):
        final_rows.add(bound_row(surface))
    inspected_paths = {item['path'] for item in graph['scripts']}
    for observation in observations:
        retained = []
        for field in observation['fields']:
            if field['key'] in {'executable', 'ExecStart', 'ExecStartPre', 'ExecStop', 'EnvironmentFile'}:
                if not all(value in inspected_paths for value in field['values']):
                    observation['unparsed'].append({'value': '<unparsed>', 'sha256': sha(json.dumps(field))})
                    continue
            retained.append(field)
        observation['fields'] = retained
        observation['acquired'] = observation['acquired'] and coverage.get(observation['surface'], False)
    return {'acquired': coverage, 'dependency_coverage': 'manual-review-required', 'observations': observations,
            'scripts': graph['scripts'], 'blocking_dependencies': final_rows.values(),
            'truncations': [{'surface': surface, 'reasons': sorted(codes)} for surface, codes in sorted(truncated.items())]}


if __name__ == '__main__':
    try:
        result = collect()
    except (OSError, ValueError, RecursionError) as error:
        result = refusal(error)
    sys.stdout.write(encode_result(result))
    sys.exit(1 if 'refusal' in result else 0)
