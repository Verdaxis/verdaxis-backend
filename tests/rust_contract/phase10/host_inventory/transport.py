"""Off-host transport. No CLI activation in stage 2a; fixtures inject the runner."""
import importlib.util
import json
import os
import re
import selectors
import signal
import subprocess
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
READER = ROOT / 'deploy/tests/host-inventory-reader.py'
EVIDENCE_ROOT = Path('/Users/jonathanjie/Documents/workspace/Verdaxis/outputs/research/rust-phase10/w0-evidence')
HOSTS = {'staging': '194.233.68.86', 'production': 'verdaxis-prod-eu'}
MAX_BYTES = 8388608


def ssh_once(argv, data):
    # One process, no retries, no shell or extra remote command. All streams are
    # bounded in memory. Failed stderr is never printed or written to evidence.
    process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    deadline = time.monotonic() + 900
    output = bytearray()
    total = 0
    offset = 0
    complete = True
    try:
        with selectors.DefaultSelector() as selector:
            for stream in (process.stdin, process.stdout, process.stderr):
                os.set_blocking(stream.fileno(), False)
            selector.register(process.stdin, selectors.EVENT_WRITE)
            selector.register(process.stdout, selectors.EVENT_READ)
            selector.register(process.stderr, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() >= deadline:
                    complete = False
                    break
                for key, _ in selector.select(timeout=0.1):
                    if key.fileobj is process.stdin:
                        try:
                            offset += os.write(process.stdin.fileno(), data[offset:offset + 4096])
                        except BrokenPipeError:
                            complete = False
                            break
                        if offset == len(data):
                            selector.unregister(process.stdin)
                            process.stdin.close()
                        continue
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    total += len(chunk)
                    if total > MAX_BYTES:
                        complete = False
                        break
                    if key.fileobj is process.stdout:
                        output.extend(chunk)
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
        return bytes(output) if complete else b'', complete and status == 0
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()


def read_host(host, runner=ssh_once):
    if host not in HOSTS:
        raise ValueError('host is not allowlisted')
    argv = ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=15',
            '-o', 'StrictHostKeyChecking=yes', HOSTS[host], 'python3 -B -I -']
    data, complete = runner(argv, READER.read_bytes())
    if len(data) > MAX_BYTES:
        return {'refusal': 'output-bytes'}
    if not data:
        return {'refusal': 'transport-error'}
    try:
        result = json.loads(data)
    except (ValueError, TypeError):
        return {'refusal': 'transport-error'}
    if isinstance(result, dict) and 'refusal' in result:
        if set(result) != {'refusal'} or not isinstance(result['refusal'], str):
            return {'refusal': 'transport-error'}
        if result['refusal'] not in {'output-bytes', 'os-error', 'value-error', 'recursion'}:
            return {'refusal': 'transport-error'}
    if isinstance(result, dict) and set(result) == {'refusal'} and result['refusal'] in {'output-bytes', 'os-error', 'value-error', 'recursion'}:
        return result
    if not complete:
        return {'refusal': 'transport-error'}
    if not isinstance(result, dict) or set(result) != {'acquired', 'dependency_coverage', 'observations', 'scripts', 'blocking_dependencies', 'truncations'}:
        raise ValueError('host inventory shape refused')
    return result


def collect_hosts(document, runner=ssh_once):
    """Attach observations to a separately reviewed register, never infer approval.

    Call only after a separate live authorization. Stage 2a exercises this with
    fixture runners; the shell CLI deliberately has no live activation path.
    """
    snapshots = {host: read_host(host, runner) for host in HOSTS}
    failures = {host: snapshot for host, snapshot in snapshots.items() if 'refusal' in snapshot}
    if failures:
        return {'host_refusals': failures}
    result = dict(document)
    result['scope'] = 'installed'
    result['dependency_coverage'] = 'manual-review-required'
    result['host_snapshots'] = snapshots
    # Raw/free-form observations are never projected into installed evidence.
    result['observations'] = []
    result['blocking_dependencies'] = [
        dict(row, host=host) for host, snapshot in snapshots.items()
        for row in snapshot['blocking_dependencies']]
    result['hosts'] = [
        {'id': host, 'owner': 'operator-review-required', 'acquired': snapshot['acquired']}
        for host, snapshot in snapshots.items()]
    import hashlib
    result['source_sha256'] = hashlib.sha256(json.dumps(snapshots, sort_keys=True).encode()).hexdigest()
    result['schedule_sha256'] = hashlib.sha256(json.dumps(result['schedules'], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
    return result


def publish(document, output_name):
    """Only the designated private folder may receive installed evidence."""
    spec = importlib.util.spec_from_file_location('w0_inventory', Path(__file__).with_name('inventory.py'))
    inventory = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(inventory)
    inventory.schema_check(document)
    if document['scope'] != 'installed' or document['binding'] != 'unsigned, digest-bound':
        raise ValueError('installed evidence scope required')
    if not re.fullmatch(r'[A-Za-z0-9_-]+\.json', output_name):
        raise ValueError('simple JSON name required')
    data = json.dumps(document, indent=2).encode() + b'\n'
    if len(data) > MAX_BYTES:
        raise ValueError('output bound exceeded')
    directory = inventory.private_directory(EVIDENCE_ROOT)
    try:
        for name in (output_name, output_name + '.sha256'):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            raise ValueError('evidence already exists')
        inventory.write_new(directory, output_name, data)
        inventory.write_new(directory, output_name + '.sha256', (inventory.digest(data) + '\n').encode())
        os.fsync(directory)
    finally:
        os.close(directory)
