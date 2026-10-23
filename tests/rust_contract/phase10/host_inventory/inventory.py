"""Bounded, off-host W0 fixture adapter and verifier. No host command execution.

Python is CI/off-host tooling here, never a Verdaxis installed dependency.
Stage 2 must add a separately reviewed transport/reader before live collection.
"""
import argparse
import hashlib
import json
import os
import re
import stat
import sys
from pathlib import Path

from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[4]
SCHEMA = ROOT / 'tests/rust_contract/phase10/host-inventory.schema.json'
MAX_BYTES = 8 * 1024 * 1024
REQUIRED = {'backup', 'restore', 'staging-pull', 'dump', 'fleet'}


def refuse(message):
    raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            refuse('duplicate JSON key')
        result[key] = value
    return result


def read_bytes(path):
    # Never traverse a symlink in any component, including evidence ancestors.
    path = Path(os.path.abspath(path))
    if any(part == 'artifact-provenance' or part == '.env' or part.endswith(('.env', '.key', '.pem', '.p12', '.pfx'))
           or part.startswith(('.env.', 'id_rsa', 'id_ed25519')) for part in path.parts):
        refuse('credential input paths are forbidden')
    current = Path(path.anchor)
    descriptors = []
    try:
        descriptor = os.open(current, os.O_RDONLY | os.O_DIRECTORY)
        descriptors.append(descriptor)
        for component in path.parts[1:-1]:
            descriptor = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                 dir_fd=descriptor)
            descriptors.append(descriptor)
        file_descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                                  dir_fd=descriptor)
        with os.fdopen(file_descriptor, 'rb') as stream:
            metadata = os.fstat(stream.fileno())
            if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_BYTES:
                refuse('input must be a bounded regular file')
            data = stream.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                refuse('input too large')
            return data
    finally:
        for descriptor in reversed(descriptors):
            os.close(descriptor)


def parse(data):
    return json.loads(data, object_pairs_hook=unique_object,
                      parse_constant=lambda _: refuse('invalid JSON constant'))


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def schema_check(document):
    schema = parse(read_bytes(SCHEMA))
    Draft202012Validator.check_schema(schema)
    if not Draft202012Validator(schema).is_valid(document):
        refuse('schema validation failed')


def redact(value):
    """Keep credential paths only; discard assignment/URL/header values wholesale.

    Free-form observations are hostile data, never commands. Broad suppression is
    deliberate: an incomplete observation is safer than preserving a secret.
    """
    if isinstance(value, dict):
        return {key: redact(item) for key, item in value.items()}
    if isinstance(value, list):
        return [redact(item) for item in value]
    if isinstance(value, str):
        if re.search(r'(?i)(password|passwd|secret|token|api.?key|authorization|bearer|cookie|private.key|://|\bEnvironment\s*=|\b[A-Z_][A-Z0-9_]*=)', value):
            return '[REDACTED]'
    return value


def validate(document):
    if isinstance(document, dict) and document.get('scope') != 'fixture':
        refuse('fixture scope required; installed acceptance is not implemented in stage 1')
    schema_check(document)
    if len(document['hosts']) != 2 or {h['id'] for h in document['hosts']} != {'production', 'staging'}:
        refuse('both hosts required')
    for host in document['hosts']:
        if not all(host['acquired'].values()):
            refuse('coverage incomplete')
    rows = document['dependencies']
    ids = [row['id'] for row in rows]
    if len(ids) != len(set(ids)):
        refuse('duplicate dependency')
    required = set(document['required_dependencies']) | REQUIRED
    if not required <= set(ids):
        refuse('required dependency missing')
    by_id = {row['id']: row for row in rows}
    for identifier, host in [('backup', 'production'), ('restore', 'production'),
                             ('dump', 'production'), ('staging-pull', 'staging')]:
        row = by_id[identifier]
        if row['kind'] != identifier or row['host'] != host or row['location'] != 'host':
            refuse('required dependency host/kind mismatch')
    fleet = by_id['fleet']
    if fleet['kind'] != 'fleet':
        refuse('fleet classification missing')
    if fleet['location'] == 'offhost':
        if fleet['host'] != 'offhost' or fleet['removed_package_dependencies']:
            refuse('fleet offhost has removed package dependency')
    for row in rows:
        if (row['host'] == 'offhost') != (row['location'] == 'offhost'):
            refuse('dependency location mismatch')
        # Explicit lists plus a static import cross-check: no self-declared clean
        # list can hide a known deleted package in the recorded imports.
        removed = row['removed_package_dependencies'] or [name for name in row['imports']
                    if name.split('.')[0] in {'app', 'scripts', 'alembic'}]
        if row['classification'] == 'unaffected' and removed:
            refuse('unaffected dependency imports removed package')
        if row['classification'] == 'replacement-required':
            refuse('NO_GO: required replacement has no owner acceptance')
        if row['classification'] == 'retirement-approved':
            # Stage 1 has no trusted retirement/owner acceptance input.
            refuse('NO_GO: retirement needs separate owner acceptance')
    if digest(canonical(document['schedules'])) != document['schedule_sha256']:
        refuse('schedule set changed')
    schedules = document['schedules']
    schedule_ids = [row['id'] for row in schedules]
    if len(schedule_ids) != len(set(schedule_ids)):
        refuse('duplicate schedule')
    for identifier in ('staging-pull', 'dump'):
        if identifier not in schedule_ids:
            refuse('required schedule missing')
    if fleet['location'] == 'offhost' and any(s['host'] == 'offhost' for s in schedules):
        refuse('offhost classification cannot create a schedule')
    # Paths are references, not contents. Do not normalize them into file reads.
    references = document['credential_references']
    for row in rows:
        if not set(row['credential_references']) <= set(references):
            refuse('credential reference not registered')
    without_references = dict(document)
    without_references.pop('credential_references')
    without_references['dependencies'] = [
        {key: value for key, value in row.items() if key != 'credential_references'} for row in rows]
    if redact(without_references) != without_references:
        refuse('unredacted secret-looking value')


def private_directory(path):
    path = Path(os.path.abspath(path))
    if path == ROOT or ROOT in path.parents:
        refuse('evidence must be outside the repository')
    # Reuse the no-follow component walk for the destination, retaining its fd.
    descriptor = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for component in path.parts[1:]:
            next_descriptor = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                                      dir_fd=descriptor)
            os.close(descriptor)
            descriptor = next_descriptor
        metadata = os.fstat(descriptor)
        if metadata.st_uid != os.getuid() or stat.S_IMODE(metadata.st_mode) != 0o700:
            refuse('evidence directory must be caller-owned mode 0700')
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def write_new(directory, name, data):
    descriptor = os.open(name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                         0o600, dir_fd=directory)
    with os.fdopen(descriptor, 'wb') as stream:
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())


def collect(args):
    raw = read_bytes(args.fixture)
    document = parse(raw)
    if not isinstance(document, dict) or document.get('scope') != 'fixture':
        refuse('fixture scope required')
    references = document.get('credential_references', [])
    row_references = [row.get('credential_references', []) for row in document.get('dependencies', [])]
    document = redact(document)
    document['credential_references'] = references
    for row, paths in zip(document['dependencies'], row_references):
        row['credential_references'] = paths
    document['source_sha256'] = digest(raw)
    document['schedule_sha256'] = digest(canonical(document['schedules']))
    validate(document)
    data = json.dumps(document, indent=2, ensure_ascii=True).encode() + b'\n'
    if len(data) > MAX_BYTES:
        refuse('output too large')
    if not re.fullmatch(r'[A-Za-z0-9_-]+\.json', args.output_name):
        refuse('output name must be a simple JSON filename')
    directory = private_directory(args.evidence_dir)
    try:
        # Refuse both pre-existing names before publishing either one. O_EXCL
        # still protects concurrent creation; partial publication fails closed.
        for name in (args.output_name, args.output_name + '.sha256'):
            try:
                os.stat(name, dir_fd=directory, follow_symlinks=False)
            except FileNotFoundError:
                continue
            refuse('evidence already exists')
        write_new(directory, args.output_name, data)
        write_new(directory, args.output_name + '.sha256', (digest(data) + '\n').encode())
        os.fsync(directory)
    finally:
        os.close(directory)
    print('W0 fixture: unsigned, digest-bound (not installed acceptance)')


def verify(args):
    raw = read_bytes(args.fixture)
    expected = read_bytes(str(args.fixture) + '.sha256').decode('ascii').strip()
    if not re.fullmatch('[a-f0-9]{64}', expected) or digest(raw) != expected:
        refuse('inventory digest mismatch')
    validate(parse(raw))
    print('W0 fixture GREEN: unsigned, digest-bound; no live acceptance')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    collector = commands.add_parser('collect')
    collector.add_argument('--fixture', required=True, type=Path)
    collector.add_argument('--evidence-dir', required=True, type=Path)
    collector.add_argument('--output-name', required=True)
    verifier = commands.add_parser('verify')
    verifier.add_argument('--fixture', required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'collect':
            collect(args)
        else:
            verify(args)
    except (ValueError, OSError, KeyError, TypeError, RecursionError):
        # Never print input, parser excerpts or raw exception values: they can
        # contain credentials. Only our constant refusal messages are exposed.
        error = sys.exc_info()[1]
        message = str(error) if type(error) is ValueError and not isinstance(error, json.JSONDecodeError) else 'invalid or inaccessible inventory'
        print('W0 refused: ' + message, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
