#!/usr/bin/env bash
# Install this reviewed helper root-owned at the fixed libexec path used by
# deploy.sh. Python 3 standard library, libpq psql and runuser are required.
# Only the private service file contains the host-generated clear password.
set -euo pipefail
set +x
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
exec /usr/bin/python3 -I - "$@" <<'PY'
import base64
import configparser
import fcntl
import hashlib
import hmac
import os
import secrets
import stat
import subprocess
import sys


def refuse(message):
    print('bundle reader refused: ' + message, file=sys.stderr)
    raise SystemExit(1)


if os.geteuid() != 0:
    refuse('root is required')
args = sys.argv[1:]
if len(args) not in (2, 3) or args[0] != '--environment' or args[1] not in ('production', 'staging'):
    refuse('usage: --environment production|staging [--check]')
check_only = len(args) == 3
if check_only and args[2] != '--check':
    refuse('unknown option')
environment = args[1]
role = 'verdaxis_bundle_reader' + ('_staging' if environment == 'staging' else '')
database = 'verdaxis_prod' if environment == 'production' else 'verdaxis_staging'
directory = '/etc/verdaxis-bundle-readers'
service = directory + '/' + environment + '-bundle-reader.pg_service.conf'
clean_env = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL': 'C'}


def private_path(path, mode, is_directory=False):
    info = os.lstat(path)
    expected = stat.S_ISDIR if is_directory else stat.S_ISREG
    if not expected(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) != mode:
        refuse('private path owner, type or mode differs')


def sql(statement, reader=False):
    command = ['/usr/bin/psql', '-XqAt', '-v', 'ON_ERROR_STOP=1']
    env = dict(clean_env)
    if reader:
        env.update(PGSERVICEFILE=service, PGSERVICE='pre_m_reader')
    else:
        command = ['/usr/sbin/runuser', '-u', 'postgres', '--'] + command + ['-d', 'postgres']
    result = subprocess.run(command, input=statement, text=True, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, env=env, timeout=30, check=False)
    if result.returncode:
        refuse('database identity or role check failed')
    return result.stdout.strip()


try:
    os.umask(0o077)
    # Do not follow a substituted directory or service-file symlink.
    etc = os.lstat('/etc')
    if not stat.S_ISDIR(etc.st_mode) or etc.st_uid != 0 or stat.S_IMODE(etc.st_mode) & 0o022:
        refuse('trusted parent differs')
    if not os.path.lexists(directory):
        if check_only:
            refuse('private service directory is missing')
        os.mkdir(directory, 0o700)
    private_path(directory, 0o700, True)
    # Serialize provisioning and checks without ever replacing the lock inode.
    lock_path = directory + '/bundle-reader.lock'
    lock_flags = os.O_RDWR | os.O_NOFOLLOW
    if not check_only:
        lock_flags |= os.O_CREAT
    lock = os.open(lock_path, lock_flags, 0o600)
    private_path(lock_path, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX)
    attributes = sql(
        "SELECT rolcanlogin AND NOT (rolsuper OR rolinherit OR rolcreatedb "
        "OR rolcreaterole OR rolreplication OR rolbypassrls) AND NOT EXISTS "
        "(SELECT 1 FROM pg_auth_members WHERE member=r.oid OR roleid=r.oid) "
        "FROM pg_roles r WHERE rolname='" + role + "';"
    )
    if attributes not in ('', 't'):
        refuse('existing reader role has unsafe attributes or memberships')
    if not os.path.lexists(service):
        if check_only or attributes:
            refuse('private service file is missing; existing roles are never rotated')
        password = secrets.token_hex(32)
        # Publish the private file before role creation. A retry after an
        # interruption uses the same password and never changes an existing one.
        temporary = service + '.new-' + secrets.token_hex(8)
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        try:
            with os.fdopen(descriptor, 'w') as output:
                output.write('[pre_m_reader]\nhost=127.0.0.1\nport=5432\ndbname=' + database + '\nuser=' + role + '\npassword=' + password + '\nconnect_timeout=5\n')
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, service)
            parent = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(parent)
            finally:
                os.close(parent)
        finally:
            if os.path.lexists(temporary):
                os.unlink(temporary)
    private_path(service, 0o600)
    config = configparser.ConfigParser(interpolation=None)
    config.read(service)
    expected = {'host': '127.0.0.1', 'port': '5432', 'dbname': database, 'user': role, 'connect_timeout': '5'}
    if config.sections() != ['pre_m_reader'] or set(config['pre_m_reader']) != set(expected) | {'password'}:
        refuse('service fields differ')
    section = config['pre_m_reader']
    if any(section[key] != value for key, value in expected.items()):
        refuse('service endpoint or role differs')
    password = section['password']
    if len(password) != 64 or any(char not in '0123456789abcdef' for char in password):
        refuse('service password format differs')
    if not attributes:
        if check_only:
            refuse('reader role is missing')
        salt = secrets.token_bytes(16)
        salted = hashlib.pbkdf2_hmac('sha256', password.encode('ascii'), salt, 4096)
        client_key = hmac.digest(salted, b'Client Key', 'sha256')
        def encode(value):
            return base64.b64encode(value).decode('ascii')
        verifier = (
            'SCRAM-SHA-256$4096:' + encode(salt) + '$'
            + encode(hashlib.sha256(client_key).digest()) + ':'
            + encode(hmac.digest(salted, b'Server Key', 'sha256'))
        )
        # No clear password enters SQL, argv, environment or server logs. Keep
        # even the SCRAM verifier out of ordinary statement/error logging.
        sql("SET log_statement=none; SET log_min_duration_statement=-1; SET log_min_duration_sample=-1; SET log_min_error_statement=panic;\nCREATE ROLE " + role + " LOGIN NOINHERIT NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS PASSWORD '" + verifier + "';")
    identity = sql(
        "SELECT current_database() || ':' || current_user || ':' "
        "|| host(inet_server_addr()) || ':' || inet_server_port();", reader=True
    )
    if identity != database + ':' + role + ':127.0.0.1:5432':
        refuse('reader connection identity differs')
except (OSError, ValueError, KeyError, configparser.Error, subprocess.SubprocessError):
    refuse('private service or database operation failed')
print('bundle reader ready: ' + environment)
PY
