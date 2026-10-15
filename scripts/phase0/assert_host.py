"""Read-only checks for the one-use Phase 0 operator checklist.

Each invocation observes current state. It grants no mutation authority and
never advances a journal. Run from the reviewed external tool installation.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import socket
import stat
import subprocess
import urllib.request
from pathlib import Path

CHECKS = ("identity", "quiesced", "outbox-empty", "tuple", "readiness", "rollback-safe", "timers")
ROOT = Path("/home/verdaxis-prod/verdaxis/prod/be")
SERVICES = (
    "verdaxis-backend.service", "verdaxis-news-refresh.service",
    "verdaxis-auth-maintenance.service", "verdaxis-product-analytics-prune.service",
    "verdaxis-order-expiry-reminders.service", "verdaxis-onboarding-attention.service",
    "verdaxis-recover.service", "verdaxis-external-recover.service",
)
TIMERS = tuple(name.replace(".service", ".timer") for name in SERVICES[1:6])


def require_outbox_drained(*, producers_stopped: bool, sequencer_stopped: bool,
                          pending_jobs: int, app_sessions: int, undispatched: int) -> None:
    if not producers_stopped or not sequencer_stopped or any(
        type(value) is not int or value != 0
        for value in (pending_jobs, app_sessions, undispatched)
    ):
        raise ValueError("Producer, sequencer and post-stop outbox drain required")


def private_bytes(path: Path) -> bytes:
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or info.st_nlink != 1 or stat.S_IMODE(info.st_mode) != 0o600:
            raise ValueError("Expected root-owned private regular file")
        value = os.read(fd, 65537)
        if len(value) > 65536:
            raise ValueError("Oversized assertion input")
        return value
    finally:
        os.close(fd)


def command(arguments: list[str]) -> str:
    return subprocess.run(arguments, check=True, capture_output=True, text=True,
                          timeout=15, env={"PATH": "/usr/bin:/bin", "LC_ALL": "C"}).stdout.strip()


def require_identity(expected: dict, actual: dict) -> None:
    keys = {"database", "database_oid", "cluster_id", "host", "port", "role", "revision"}
    if set(expected) != keys or set(actual) != keys or expected != actual:
        raise ValueError("Database identity differs from approved tuple")
    if expected["database"] not in {"verdaxis", "verdaxis_prod"} or expected["role"] != "verdaxis_backup":
        raise ValueError("Only the exact read-only production identity is accepted")
    if (expected["host"] != "127.0.0.1" or type(expected["port"]) is not int
            or not 1024 < expected["port"] < 65536):
        raise ValueError("Expected numeric loopback endpoint")
    if (type(expected["database_oid"]) is not int or expected["database_oid"] <= 0
            or not isinstance(expected["cluster_id"], str)
            or re.fullmatch(r"[1-9][0-9]*", expected["cluster_id"]) is None):
        raise ValueError("Expected exact database and cluster identifiers")
    revision = "p0_20261016_union" if expected["database"] == "verdaxis_prod" else "kycg_20261014_prod"
    if expected["revision"] != revision:
        raise ValueError("Unapproved Phase 0 revision")


def database_facts(manifest: dict) -> tuple[dict, int, int]:
    # A libpq passfile is separate from the manifest and never enters argv.
    import psycopg2
    expected = manifest["identity"]
    require_identity(expected, expected)
    password = private_bytes(Path(manifest["password_file"])).decode().rstrip("\n")
    with psycopg2.connect(host=expected["host"], port=expected["port"],
                          dbname=expected["database"], user="verdaxis_backup",
                          password=password, connect_timeout=5,
                          options="-c default_transaction_read_only=on -c statement_timeout=5000") as connection:
        with connection.cursor() as cursor:
            cursor.execute("SELECT current_database(), oid FROM pg_database WHERE datname=current_database()")
            database, oid = cursor.fetchone()
            cursor.execute("SELECT current_user, inet_server_addr()::text, inet_server_port()")
            role, host, port = cursor.fetchone()
            cursor.execute("""SELECT rolcanlogin AND NOT rolinherit AND NOT rolsuper
                AND NOT rolcreatedb AND NOT rolcreaterole AND NOT rolreplication
                AND NOT rolbypassrls AND NOT EXISTS
                    (SELECT 1 FROM pg_auth_members WHERE member=pg_roles.oid)
                FROM pg_roles WHERE rolname=current_user""")
            if cursor.fetchone() != (True,):
                raise ValueError("Unsafe backup role properties")
            cursor.execute("SELECT version_num FROM alembic_version")
            revisions = cursor.fetchall()
            if len(revisions) != 1:
                raise ValueError("Expected one revision")
            # Cluster identity is read independently as the local operator. Do
            # not grant pg_control_system access to runtime or backup roles.
            cluster = command(["/usr/bin/sudo", "-n", "-u", "postgres", "/usr/bin/psql",
                               "-XAt", "-h", "/var/run/postgresql",
                               "-p", str(expected["port"]), "-d", "postgres",
                               "-c", "SELECT system_identifier FROM pg_control_system()"])
            actual = dict(database=database, database_oid=oid, cluster_id=cluster,
                          host=host, port=port, role=role, revision=revisions[0][0])
            require_identity(expected, actual)
            cursor.execute("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='verdaxis_app'")
            sessions = cursor.fetchone()[0]
            cursor.execute("SELECT count(*) FROM market_event_outbox WHERE stream_seq IS NULL")
            count = cursor.fetchone()[0]
    return actual, sessions, count


def require_stopped_unit(output: str) -> None:
    fields = dict(line.split("=", 1) for line in output.splitlines())
    if set(fields) != {"ActiveState", "SubState", "Job"}:
        raise ValueError("Incomplete systemd observation")
    if (fields["ActiveState"] not in {"inactive", "failed"}
            or fields["SubState"] not in {"dead", "failed"}
            or fields["Job"] not in {"", "0"}):
        raise ValueError("A writer, timer or pending job remains")


def stopped_units() -> None:
    for unit in SERVICES + TIMERS:
        require_stopped_unit(command([
            "/usr/bin/systemctl", "show", unit,
            "--property=ActiveState,SubState,Job",
        ]))


def verify_tuple(manifest: dict) -> None:
    sha = manifest["release_sha"]
    if re.fullmatch(r"[0-9a-f]{40}", sha) is None:
        raise ValueError("Full release SHA required")
    if ROOT.is_symlink() or command(["/usr/bin/git", "-C", str(ROOT), "rev-parse", "HEAD"]) != sha:
        raise ValueError("Checkout identity differs")
    if command(["/usr/bin/git", "-C", str(ROOT), "status", "--porcelain", "--untracked-files=no"]):
        raise ValueError("Tracked source is dirty")
    artifacts = manifest["artifacts"]
    required = {str(ROOT / ".env"), str(ROOT / ".runtime-release.env"),
                str(ROOT / ".runtime-monitor.env"),
                "/etc/systemd/system/verdaxis-backend.service",
                "/etc/systemd/system/verdaxis-backend.service.d/90-phase0-fence.conf"}
    if (not required.issubset(artifacts)
            or manifest.get("monitor_identity_file") != str(ROOT / ".runtime-monitor.env")):
        raise ValueError("Complete environment/unit/monitor digests required")
    identity = f"ENVIRONMENT=production\nRELEASE_SHA={sha}\n"
    if (ROOT / ".runtime-monitor.env").read_text() != identity:
        raise ValueError("Monitor identity differs")
    revision = manifest["identity"]["revision"]
    if (ROOT / ".runtime-release.env").read_text() != identity + f"MIGRATION_REVISION={revision}\n":
        raise ValueError("Runtime identity differs")
    for name, digest in artifacts.items():
        path = Path(name)
        if not path.is_absolute() or path.is_symlink() or re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError("Invalid artifact")
        if hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError("Tuple artifact differs")


def run_check(manifest: dict, check: str) -> None:
    if socket.gethostname().split(".")[0] != manifest["hostname"]:
        raise ValueError("Wrong host")
    if check in {"identity", "outbox-empty", "rollback-safe"}:
        if check != "identity":
            stopped_units()
        _, sessions, count = database_facts(manifest)
        if check != "identity":
            stopped_units()
            require_outbox_drained(producers_stopped=True, sequencer_stopped=True,
                                   pending_jobs=0, app_sessions=sessions, undispatched=count)
        if check == "rollback-safe":
            # A hash report is operator-reviewed evidence, never a boolean claim
            # that customer writes did not occur. Reopening invalidates it.
            if hashlib.sha256(private_bytes(Path(manifest["reconciliation_file"]))).hexdigest() != manifest["reconciliation_sha256"]:
                raise ValueError("Internal-write reconciliation evidence changed")
    elif check in {"quiesced", "timers"}:
        stopped_units()
    elif check == "tuple":
        verify_tuple(manifest)
    elif check == "readiness":
        verify_tuple(manifest)
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        with opener.open("http://127.0.0.1:8000/health/ready", timeout=3) as response:
            payload = json.loads(response.read(4097))
        if payload != dict(status="ok", db="ok", environment="production", release_sha=manifest["release_sha"]):
            raise ValueError("Readiness differs from exact release")
    else:
        raise ValueError("Unknown check")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--check", choices=CHECKS, required=True)
    args = parser.parse_args()
    try:
        manifest = json.loads(private_bytes(args.manifest))
        run_check(manifest, args.check)
    except Exception:
        # Database exceptions can contain credentials or private rows.
        print(f"{args.check}: refused; inspect private operator evidence")
        return 1
    print(f"{args.check}: verified")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
