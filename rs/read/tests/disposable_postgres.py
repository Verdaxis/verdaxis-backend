"""Standard-library helpers for local disposable PostgreSQL tests."""

import ipaddress
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import unquote, urlsplit

ROOT = Path(__file__).resolve().parents[3]
IMAGE = "postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272"
PASSWORD = "disposable_test_pw"
APP_ROLE = "verdaxis_app_staging"
MIGRATOR_ROLE = "verdaxis_migrator_staging"
BACKUP_ROLE = "verdaxis_backup_staging"


def docker(*arguments, **kwargs):
    return subprocess.run(["docker", *arguments], check=True, text=True, **kwargs)


def literal(value):
    if value is None:
        return "NULL"
    if isinstance(value, bool):
        return "TRUE" if value else "FALSE"
    if hasattr(value, "isoformat"):
        value = value.isoformat()
    return "'" + str(value).replace("'", "''") + "'"


def bind_sql(statement, values):
    return re.sub(r"(?<!:):([a-z_][a-z_0-9]*)", lambda match: literal(values[match[1]]), statement)


def psql(url, statement):
    parsed = urlsplit(url)
    if parsed.scheme not in {"postgres", "postgresql"}:
        raise ValueError("a PostgreSQL URL is required")
    if not ipaddress.ip_address(parsed.hostname).is_loopback:
        raise ValueError("only numeric loopback databases are allowed")
    if not parsed.path.endswith("_test") or parsed.query or parsed.fragment:
        raise ValueError("a disposable database ending in _test is required")
    arguments = ["psql", "-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1",
                 "-h", parsed.hostname, "-p", str(parsed.port or 5432),
                 "-U", unquote(parsed.username or ""), "-d", parsed.path[1:]]
    password = unquote(parsed.password or "")
    container = os.environ.get("RUST_TEST_POSTGRES_CONTAINER")
    if container:
        # The published host port maps to the container's fixed database port.
        arguments[arguments.index("-p") + 1] = "5432"
        command = ["docker", "exec", "-i", "-e", f"PGPASSWORD={password}", container, *arguments]
        environment = os.environ.copy()
    else:
        command = arguments
        environment = {**os.environ, "PGPASSWORD": password}
    result = subprocess.run(command, input=statement, text=True,
                            capture_output=True, env=environment)
    if result.returncode:
        print(result.stderr, file=sys.stderr, end="")
        result.check_returncode()
    return result.stdout.strip()


def rows(url, statement):
    result = psql(url, f"SELECT COALESCE(json_agg(row_to_json(fixture_rows)), '[]'::json) FROM ({statement}) fixture_rows;")
    return json.loads(result)


class DisposablePostgres:
    def __init__(self, database):
        if not re.fullmatch(r"[a-z_]+_test", database):
            raise ValueError("invalid disposable database name")
        self.database = database
        self.container = None
        self.port = None
        self.release_sha = None

    def url(self, role, password=PASSWORD):
        return f"postgresql://{role}:{password}@127.0.0.1:{self.port}/{self.database}"

    def policy(self, name):
        directory = "/tmp/rust-role-policy"
        docker("exec", self.container, "mkdir", "-p", directory)
        for filename in ("app_acl_policy.sql", "converge_runtime_object_acls.sql",
                         "bootstrap_roles.sql", "validate_roles.sql"):
            path = ROOT / "deploy/postgres" / filename
            docker("cp", str(path), f"{self.container}:{directory}/{filename}", capture_output=True)
        docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", self.database,
               "-v", "ON_ERROR_STOP=1", "-v", f"database_name={self.database}",
               "-v", f"app_role={APP_ROLE}", "-v", f"migrator_role={MIGRATOR_ROLE}",
               "-v", f"backup_role={BACKUP_ROLE}", "-f", f"{directory}/{name}")

    def __enter__(self):
        try:
            self.container = docker("run", "--platform", "linux/amd64", "-d", "--rm",
                                    "-e", "POSTGRES_PASSWORD=disposable_admin_pw",
                                    "-p", "127.0.0.1::5432", IMAGE, capture_output=True).stdout.strip()
            for _ in range(90):
                ready = subprocess.run(["docker", "exec", self.container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if ready.returncode == 0:
                    break
                time.sleep(1)
            else:
                raise RuntimeError("disposable PostgreSQL did not become ready")
            self.port = int(docker("port", self.container, "5432/tcp", capture_output=True).stdout.strip().rsplit(":", 1)[1])
            docker("exec", "-i", self.container, "psql", "-X", "-U", "postgres", "-v", "ON_ERROR_STOP=1",
                   input=f"CREATE DATABASE {self.database};\n\\connect {self.database}\nCREATE EXTENSION postgis;\n")
            self.policy("bootstrap_roles.sql")
            docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", self.database,
                   "-v", "ON_ERROR_STOP=1", "-c",
                   f"ALTER ROLE {MIGRATOR_ROLE} PASSWORD '{PASSWORD}'; ALTER ROLE {APP_ROLE} PASSWORD '{PASSWORD}';")
            self.release_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            environment = {**os.environ, "SQLX_OFFLINE": "true",
                           "RELEASE_SHA": self.release_sha,
                           "RUST_RUNNER_ENVIRONMENT": "staging",
                           "RUST_RUNNER_MIGRATOR_DATABASE_URL": self.url(MIGRATOR_ROLE)}
            subprocess.run(["cargo", "test", "--manifest-path", str(ROOT / "rs/Cargo.toml"), "--locked",
                            "-p", "verdaxis-read", "--features", "contract-tests", "--test", "phase10_migrate_postgres",
                            "runner_database_adopts_to_the_baseline", "--", "--ignored"],
                           cwd=ROOT, env=environment, check=True)
            self.policy("bootstrap_roles.sql")
            # The frozen baseline omits privileges. Restore the historical
            # trigger-function boundary before the unchanged role validator.
            psql(self.url(MIGRATOR_ROLE),
                 "REVOKE ALL ON FUNCTION public.market_event_outbox_advance_replay_floor() FROM PUBLIC; "
                 "REVOKE ALL ON FUNCTION public.market_event_outbox_refuse_truncate() FROM PUBLIC;")
            self.policy("validate_roles.sql")
            return self
        except BaseException:
            self.cleanup()
            raise

    def cleanup(self):
        if self.container:
            subprocess.run(["docker", "rm", "-f", "-v", self.container], check=False,
                           stdout=subprocess.DEVNULL)
            self.container = None

    def __exit__(self, exc_type, exc_value, traceback):
        self.cleanup()
