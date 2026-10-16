"""Strict host authority inputs; validation alone grants no execution permission.

The CLI integration must also attest the installed bytes, independent cluster
observations, isolation and bootstrap evidence. These primitives do not enable
production mode in bootstrap, copy or verify.
"""
from __future__ import annotations

import hashlib
import ipaddress
import json
import os
import re
import stat
from pathlib import Path

from scripts.phase0.manifest import PROD_REVISION, PROD_SHA, digest

IDENTITY_FIELDS = {
    'host', 'port', 'database', 'oid', 'role', 'server_host', 'server_port',
    'cluster_system_identifier',
}
CONTRACT_FIELDS = {
    'format_version', 'mode', 'tool', 'approved', 'hostname', 'release_sha',
    'source_sha', 'source_revision', 'target_revision', 'source', 'target',
    'tool_tree_sha256', 'source_catalog_sha256', 'frozen_source_sha256',
    'isolation_sha256', 'bootstrap_receipt_sha256',
}
# Include all local executable/configuration inputs, not only the CLI module.
# Package/venv provenance remains a separate installation gate.
TREE_DIRECTORIES = ('scripts', 'app', 'alembic', 'deploy')
TREE_FILES = ('alembic.ini', 'requirements.txt', 'pyproject.toml')
MAX_AUTHORITY_BYTES = 8_000_000


def parse_json(contents):
    def unique_pairs(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('Duplicate authority key')
            result[key] = value
        return result

    def refuse_constant(_value):
        raise ValueError('Nonfinite authority value')

    try:
        value = json.loads(contents, object_pairs_hook=unique_pairs,
                           parse_constant=refuse_constant)
    except (TypeError, UnicodeError, json.JSONDecodeError):
        raise ValueError('Invalid authority JSON') from None
    if not isinstance(value, dict):
        raise ValueError('Authority must be an object')
    return value


def read_root_private(path):
    """Walk root-controlled directories using descriptors, never follow links."""
    path = Path(path)
    if not path.is_absolute() or '..' in path.parts:
        raise ValueError('Absolute private authority path required')
    directory = None
    descriptor = None
    try:
        directory = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        for part in path.parts[1:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW,
                            dir_fd=directory)
            os.close(directory)
            directory = child
            info = os.fstat(directory)
            if info.st_uid != 0 or info.st_mode & 0o022:
                raise ValueError('Authority parent is not root controlled')
        parent = os.fstat(directory)
        if parent.st_uid != 0 or stat.S_IMODE(parent.st_mode) != 0o700:
            raise ValueError('Authority directory must be root-owned mode 0700')
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=directory)
        info = os.fstat(descriptor)
        if (not stat.S_ISREG(info.st_mode) or info.st_uid != 0
                or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600
                or info.st_size > MAX_AUTHORITY_BYTES):
            raise ValueError('Root-owned single-link mode-0600 authority required')
        with os.fdopen(descriptor, 'rb', closefd=False) as stream:
            contents = stream.read(MAX_AUTHORITY_BYTES + 1)
        if len(contents) > MAX_AUTHORITY_BYTES:
            raise ValueError('Authority input too large')
        after = os.fstat(descriptor)
        if (info.st_size, info.st_mtime_ns, info.st_ctime_ns) != (
                after.st_size, after.st_mtime_ns, after.st_ctime_ns):
            raise ValueError('Authority changed while reading')
        return contents
    except OSError:
        raise ValueError('Private host authority could not be opened') from None
    finally:
        if descriptor is not None:
            os.close(descriptor)
        if directory is not None:
            os.close(directory)


def validate_identity(identity, *, side):
    if not isinstance(identity, dict) or set(identity) != IDENTITY_FIELDS:
        raise ValueError('Exact database identity fields required')
    expected = ('verdaxis', 'verdaxis_backup') if side == 'source' else (
        'verdaxis_prod', 'verdaxis_migrator')
    if (identity['database'], identity['role']) != expected:
        raise ValueError('Production database or role differs')
    if identity['host'] != '127.0.0.1':
        raise ValueError('Numeric loopback endpoint required')
    for key in ('port', 'server_port'):
        if type(identity[key]) is not int or not 1024 < identity[key] < 65536:
            raise ValueError('Exact PostgreSQL port required')
    if type(identity['oid']) is not int or not 0 < identity['oid'] <= 4294967295:
        raise ValueError('Exact database OID required')
    cluster = identity['cluster_system_identifier']
    if not isinstance(cluster, str) or not re.fullmatch(r'[1-9][0-9]*', cluster):
        raise ValueError('Exact cluster system identifier required')
    if int(cluster) > 18446744073709551615:
        raise ValueError('Invalid cluster system identifier')
    try:
        address = ipaddress.ip_interface(identity['server_host'])
    except (ValueError, TypeError):
        raise ValueError('Numeric server endpoint required') from None
    if not address.ip.is_loopback or address.network.prefixlen != address.max_prefixlen:
        raise ValueError('Host server endpoint must be loopback')


def validate_contract(manifest, *, tool):
    if not isinstance(manifest, dict) or set(manifest) != CONTRACT_FIELDS:
        raise ValueError('Exact host authority fields required')
    if (type(manifest['format_version']) is not int or manifest['format_version'] != 1
            or manifest['mode'] != 'production' or manifest['approved'] is not True
            or tool not in ('bootstrap', 'copy', 'verify') or manifest['tool'] != tool):
        raise ValueError('Explicit tool-specific production authority required')
    if not isinstance(manifest['hostname'], str) or not re.fullmatch(
            r'[A-Za-z0-9][A-Za-z0-9.-]{0,252}', manifest['hostname']):
        raise ValueError('Exact host name required')
    if (not isinstance(manifest['release_sha'], str)
            or not re.fullmatch(r'[0-9a-f]{40}', manifest['release_sha'])
            or manifest['source_sha'] != PROD_SHA
            or manifest['source_revision'] != PROD_REVISION
            or manifest['target_revision'] != 'p0_20261016_union'):
        raise ValueError('Approved release and frozen source revisions required')
    for key in CONTRACT_FIELDS:
        if key.endswith('_sha256'):
            value = manifest[key]
            if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value):
                raise ValueError('Exact evidence digest required')
    for side in ('source', 'target'):
        validate_identity(manifest[side], side=side)
    source, target = manifest['source'], manifest['target']
    if source['oid'] == target['oid']:
        raise ValueError('Source and target must be distinct databases')
    for key in ('host', 'port', 'server_host', 'server_port', 'cluster_system_identifier'):
        if source[key] != target[key]:
            raise ValueError('One independently attested host cluster required')


def tree_digest(root):
    """Hash the complete tool/release input tree, including untracked files.

    A clean committed-tree comparison and trusted installation ownership must
    accompany this digest. A self-reported digest is not release approval.
    """
    root = Path(root)
    files = {}

    def record(path):
        info = path.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise ValueError('Release input is not a single-link regular file')
        files[str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()

    for name in TREE_FILES:
        path = root / name
        if os.path.lexists(path):
            record(path)
    for name in TREE_DIRECTORIES:
        directory = root / name
        if not os.path.lexists(directory):
            continue
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError('Release directory is not a real directory')
        for parent, directories, filenames in os.walk(directory, followlinks=False):
            for child in directories:
                if (Path(parent) / child).is_symlink():
                    raise ValueError('Release directory symlink refused')
            # Bytecode is not a source input. Production installation must disable
            # bytecode writes and validate its interpreter/package installation.
            directories[:] = [child for child in directories if child != '__pycache__']
            for filename in filenames:
                record(Path(parent) / filename)
    return digest(files)
