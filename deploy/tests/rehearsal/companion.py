"""Prepare the second real old runtime and retained local monitor inputs."""

from __future__ import annotations

import gzip
import hashlib
import json
import os
import re
import secrets
import shutil
import subprocess
import time
from datetime import datetime, timezone
from pathlib import Path


def _run(*args, data=None, env=None, cwd=None):
    result = subprocess.run(
        [str(arg) for arg in args],
        input=data,
        env=env,
        cwd=cwd,
        capture_output=True,
        check=False,
    )
    if result.returncode:
        # Only the caller's private preparation log receives command diagnostics.
        raise RuntimeError(
            f"companion command failed: {args[0]}: {result.stderr.decode(errors='replace')}"
        )
    return result.stdout


def _write(path, content, mode=0o600):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not isinstance(content, str):
        content = json.dumps(content, sort_keys=True, indent=2) + "\n"
    path.write_text(content)
    path.chmod(mode)


def _sql(database, query):
    return (
        _run(
            "runuser",
            "-u",
            "postgres",
            "--",
            "psql",
            "-XqAt",
            "-v",
            "ON_ERROR_STOP=1",
            "-d",
            database,
            data=query.encode(),
        )
        .decode()
        .strip()
    )


def _ready(environment, port, release):
    expected = {
        "status": "ok",
        "db": "ok",
        "environment": environment,
        "release_sha": release,
    }
    for _ in range(90):
        try:
            observed = json.loads(
                _run(
                    "curl",
                    "--fail",
                    "--silent",
                    "--show-error",
                    "--max-time",
                    "2",
                    f"http://127.0.0.1:{port}/health/ready",
                )
            )
            if observed == expected:
                return observed
        except (RuntimeError, ValueError):
            pass
        time.sleep(1)
    raise RuntimeError(f"actual {environment} readiness did not arrive")


def _old_python_backends(old_sha):
    observations = []
    for environment, suffix, port in (
        ("production", "", 8000),
        ("staging", "-staging", 8001),
    ):
        checkout = (
            Path("/home/verdaxis-prod/verdaxis")
            / ("prod" if environment == "production" else "staging")
            / "be"
        )
        launcher = checkout / "venv/bin/uvicorn"
        content = launcher.read_text()
        first, separator, body = content.partition("\n")
        if not separator or not first.startswith("#!") or "python" not in first:
            raise RuntimeError(
                "actual Uvicorn entrypoint is not a Python package launcher"
            )
        # Relocate the copied environment's real package entrypoint, not its code.
        launcher.write_text(f"#!{checkout}/venv/bin/python\n" + body)
        launcher.chmod(0o700)
        _run("chown", "verdaxis-prod:verdaxis-prod", launcher)
        unit = f"verdaxis-backend{suffix}.service"
        _run("systemctl", "start", unit)
        observed = _ready(environment, port, old_sha)
        properties = {}
        for name in ("MainPID", "InvocationID"):
            properties[name] = (
                _run("systemctl", "show", unit, f"--property={name}", "--value")
                .decode()
                .strip()
            )
        if int(properties["MainPID"]) <= 1 or not re.fullmatch(
            "[0-9a-f]{32}", properties["InvocationID"]
        ):
            raise RuntimeError("old Python backend has no actual process invocation")
        observations.append(
            {
                "unit": unit,
                "ready": observed,
                "pid": int(properties["MainPID"]),
                "invocation_id": properties["InvocationID"],
            }
        )
        _run("systemctl", "stop", unit)
        _run("systemctl", "disable", unit)
        if (
            _run("systemctl", "show", unit, "--property=ActiveState", "--value")
            .decode()
            .strip()
            != "inactive"
        ):
            raise RuntimeError("old Python backend did not stop before alias binding")
    return observations


def _aliases(old_sha):
    units = []
    for environment, public_port, rust_port in (
        ("production", 8000, 8010),
        ("staging", 8001, 8011),
    ):
        name = f"rehearsal-api-{environment}"
        _write(
            f"/etc/systemd/system/{name}.socket",
            f"""[Unit]
Description=Disposable loopback listener for the actual {environment} Rust service
[Socket]
ListenStream=127.0.0.1:{public_port}
NoDelay=true
[Install]
WantedBy=sockets.target
""",
            0o644,
        )
        _write(
            f"/etc/systemd/system/{name}.service",
            f"""[Unit]
Description=Disposable socket forwarding to actual {environment} Rust
[Service]
ExecStart=/usr/lib/systemd/systemd-socket-proxyd 127.0.0.1:{rust_port}
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
""",
            0o644,
        )
        units.extend([f"{name}.socket", f"{name}.service"])
    _run("systemctl", "daemon-reload")
    for environment in ("production", "staging"):
        _run("systemctl", "start", f"rehearsal-api-{environment}.socket")
        _ready(environment, 8000 if environment == "production" else 8001, old_sha)
    return units


def _monitors(checkout, old_sha):
    source = checkout / "deploy/monitor"
    destination = Path("/usr/local/libexec/verdaxis-monitor")
    destination.mkdir(parents=True, exist_ok=True)
    destination.chmod(0o755)
    manifest = json.loads((source / "artifact-manifest.json").read_text())
    for artifact in manifest["artifacts"]:
        source_file = source / artifact["source"]
        if hashlib.sha256(source_file.read_bytes()).hexdigest() != artifact["sha256"]:
            raise RuntimeError("old monitor source differs from its immutable manifest")
        target = Path(artifact["destination"])
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_file, target)
        target.chmod(int(artifact["mode"], 8))
    for kind, target in (
        ("sysusers", "/etc/sysusers.d"),
        ("tmpfiles", "/etc/tmpfiles.d"),
    ):
        shutil.copyfile(
            source / f"verdaxis-monitor.{kind}", Path(target) / "verdaxis-monitor.conf"
        )
        (Path(target) / "verdaxis-monitor.conf").chmod(0o644)
    _run("systemd-sysusers", "/etc/sysusers.d/verdaxis-monitor.conf")
    _run("systemd-tmpfiles", "--create", "/etc/tmpfiles.d/verdaxis-monitor.conf")
    units = []
    for unit in source.iterdir():
        if unit.suffix in (".service", ".timer"):
            shutil.copyfile(unit, Path("/etc/systemd/system") / unit.name)
            (Path("/etc/systemd/system") / unit.name).chmod(0o644)
            units.append(unit.name)
    for environment in ("production", "staging"):
        _write(
            f"/etc/verdaxis-monitor/runtime-identities/{environment}.identity",
            f"ENVIRONMENT={environment}\nRELEASE_SHA={old_sha}\n",
            0o644,
        )
    # Retain the old external source bytes for snapshot and fencing only. Its
    # broad remote probes must never run as part of disposable preparation.
    external = checkout / "deploy/external_monitor"
    for source_name, installed_name in (
        ("verdaxis_monitor.py", "verdaxis-monitor"),
        ("verdaxis_recover.py", "verdaxis-recover"),
        ("verdaxis_autodiag.py", "verdaxis-autodiag"),
    ):
        target = Path("/usr/local/sbin") / installed_name
        shutil.copyfile(external / source_name, target)
        target.chmod(0o755)
    for unit in (external / "systemd").iterdir():
        if unit.suffix in (".service", ".timer"):
            shutil.copyfile(unit, Path("/etc/systemd/system") / unit.name)
            (Path("/etc/systemd/system") / unit.name).chmod(0o644)
            units.append(unit.name)
    _run("systemctl", "daemon-reload")
    # No legacy monitor timer is started here. Mask autonomous recovery before
    # any later controlled monitor invocation can emit OnFailure dependencies.
    for name in (
        "verdaxis-recover.service",
        "verdaxis-external-recover.service",
        "verdaxis-codex-diagnose.service",
    ):
        installed = Path("/etc/systemd/system") / name
        # Keep the original bytes in the immutable old checkout. A runtime mask
        # cannot override a higher-priority unit installed under /etc.
        installed.unlink()
        _run("systemctl", "mask", name)
        if (
            _run("systemctl", "show", name, "--property=LoadState", "--value")
            .decode()
            .strip()
            != "masked"
        ):
            raise RuntimeError("legacy recovery was not effectively masked")
    return units


def _backups():
    directory = Path("/home/verdaxis-prod/backups")
    attempts = directory / "attempts"
    attempts.mkdir(parents=True)
    directory.chmod(0o750)
    attempts.chmod(0o750)
    # umami is a genuine separate disposable database, not a fabricated dump.
    _sql("postgres", "CREATE DATABASE umami;")
    _sql(
        "umami",
        "CREATE TABLE rehearsal_backup_marker (created_at timestamptz NOT NULL DEFAULT now()); INSERT INTO rehearsal_backup_marker DEFAULT VALUES;",
    )
    started = datetime.now(timezone.utc)
    backup_id = started.strftime("%Y%m%d-%H%M%S")
    attempt_id = f"attempt-{backup_id}"
    attempt = {
        "schema_version": 1,
        "attempt_id": attempt_id,
        "state": "started",
        "started_at": started.isoformat(),
        "finished_at": None,
        "backup_id": None,
    }
    _write(attempts / f"{attempt_id}.json", attempt, 0o640)
    records = []
    for database, prefix in (
        ("verdaxis_prod", "verdaxis-prod"),
        ("verdaxis_staging", "verdaxis-staging"),
        ("umami", "umami"),
    ):
        path = directory / f"{prefix}-{backup_id}.sql.gz"
        # Stream the real dump. Do not hold a database-sized buffer in Python.
        with (
            path.open("wb") as output,
            gzip.GzipFile(fileobj=output, mode="wb") as compressed,
            subprocess.Popen(
                [
                    "runuser",
                    "-u",
                    "postgres",
                    "--",
                    "/usr/lib/postgresql/17/bin/pg_dump",
                    "--no-password",
                    database,
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
            ) as process,
        ):
            shutil.copyfileobj(process.stdout, compressed)
            diagnostic = process.stderr.read()
            if process.wait():
                raise RuntimeError(
                    "real pg_dump failed: " + diagnostic.decode(errors="replace")
                )
        path.chmod(0o640)
        records.append(
            {
                "database": database,
                "path": str(path),
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
        )
    completed = datetime.now(timezone.utc)
    # The backup identifier denotes the completion second in the verifier contract.
    completed_id = completed.strftime("%Y%m%d-%H%M%S")
    for row in records:
        old_path = Path(row["path"])
        new_path = old_path.with_name(old_path.name.replace(backup_id, completed_id))
        old_path.rename(new_path)
        os.utime(new_path, (completed.timestamp(), completed.timestamp()))
        row["path"] = str(new_path)
    attempt.update(
        state="succeeded", finished_at=completed.isoformat(), backup_id=completed_id
    )
    _write(attempts / f"{attempt_id}.json", attempt, 0o640)
    _write(
        directory / "status.json",
        {
            "ok": True,
            "completed_at": completed.isoformat(),
            "backup_id": completed_id,
            "databases": ["verdaxis_prod", "verdaxis_staging", "umami"],
        },
        0o640,
    )
    _run("chown", "-R", "verdaxis-prod:verdaxis-backup-readers", directory)
    _run("setfacl", "-m", "u:verdaxis-backup-reader:--x", "/home/verdaxis-prod")
    return records


def _preserved_schedules():
    """Install explicitly modeled local schedules, not recovered host policy."""
    base = Path("/var/lib/verdaxis-rehearsal-preserved")
    base.mkdir(mode=0o700)
    marker = base / "enabled"
    _write(marker, "modeled-disposable-only\n")
    backup = Path("/usr/local/sbin/verdaxis-rehearsal-backup")
    pull = Path("/usr/local/sbin/verdaxis-rehearsal-staging-pull")
    gate = """#!/bin/bash
set -euo pipefail
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
umask 077
[[ $EUID == 0 && -f /run/verdaxis-disposable-rehearsal && ! -L /run/verdaxis-disposable-rehearsal ]]
[[ $(stat -c '%u:%a' /run/verdaxis-disposable-rehearsal) == 0:644 ]]
[[ -f /var/lib/verdaxis-rehearsal-preserved/enabled ]]
"""
    _write(
        backup,
        gate
        + """exec 9>/var/lib/verdaxis-rehearsal-preserved/backup.lock
flock -n 9
work=$(mktemp -d /var/lib/verdaxis-rehearsal-preserved/.dump.XXXXXXXX)
trap 'rm -rf -- "$work"' EXIT
for database in verdaxis_prod verdaxis_staging umami; do
    runuser -u postgres -- /usr/lib/postgresql/17/bin/pg_dump --no-password "$database" | gzip -c > "$work/$database.sql.gz"
    gzip --test "$work/$database.sql.gz"
done
mkdir -p /var/lib/verdaxis-rehearsal-preserved/production
for database in verdaxis_prod verdaxis_staging umami; do
    mv -T "$work/$database.sql.gz" "/var/lib/verdaxis-rehearsal-preserved/production/$database.sql.gz"
done
""",
        0o700,
    )
    _write(
        pull,
        gate
        + """exec 9>/var/lib/verdaxis-rehearsal-preserved/backup.lock
flock -n 9
mkdir -p /var/lib/verdaxis-rehearsal-preserved/staging
for database in verdaxis_prod verdaxis_staging umami; do
    source=/var/lib/verdaxis-rehearsal-preserved/production/$database.sql.gz
    [[ -f $source && ! -L $source ]]
    install -m 0600 "$source" "/var/lib/verdaxis-rehearsal-preserved/staging/$database.sql.gz"
    cmp -- "$source" "/var/lib/verdaxis-rehearsal-preserved/staging/$database.sql.gz"
done
""",
        0o700,
    )
    cron = Path("/etc/cron.d/verdaxis-rehearsal-preserved")
    _write(
        cron,
        "# Modeled disposable local schedules; not installed-host evidence.\n"
        "0 3 * * * root /usr/local/sbin/verdaxis-rehearsal-staging-pull\n"
        "0 4 * * * root /usr/local/sbin/verdaxis-rehearsal-backup\n",
        0o644,
    )
    service = Path("/etc/systemd/system/verdaxis-local-db-backup.service")
    timer = Path("/etc/systemd/system/verdaxis-local-db-backup.timer")
    _write(
        service,
        "[Unit]\nDescription=Modeled disposable native local database backup\n"
        "[Service]\nType=oneshot\nExecStart=/usr/local/sbin/verdaxis-rehearsal-backup\n",
        0o644,
    )
    _write(
        timer,
        "[Unit]\nDescription=Modeled disposable 03:00 Singapore local backup schedule\n"
        "[Timer]\nOnCalendar=*-*-* 03:00:00 Asia/Singapore\nPersistent=true\n"
        "[Install]\nWantedBy=timers.target\n",
        0o644,
    )
    _run("systemctl", "daemon-reload")
    _run("systemctl", "start", "verdaxis-local-db-backup.service")
    if (
        _run(
            "systemctl",
            "show",
            "verdaxis-local-db-backup.service",
            "--property=Result",
            "--value",
        )
        .decode()
        .strip()
        != "success"
    ):
        raise RuntimeError("modeled actual database backup service did not succeed")
    _run("bash", pull)
    return {
        "scope": "modeled-disposable",
        "staging_pull_hour": 3,
        "backup_hour": 4,
        "paths": {
            str(path): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in (cron, backup, pull, service, timer)
        },
    }


def prepare(
    root,
    private,
    current_checkout,
    selected_environment,
    old_sha,
    previous_binary_path,
    password,
):
    """Create the other old environment; return only structural observations."""
    root, private, current_checkout = map(Path, (root, private, current_checkout))
    if (
        os.geteuid() != 0
        or os.environ.get("CI") != "true"
        or os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
        or Path("/proc/1/comm").read_text().strip() != "systemd"
    ):
        raise RuntimeError("companion preparation requires the disposable hosted VM")
    if selected_environment not in ("production", "staging") or not re.fullmatch(
        "[0-9a-f]{40}", old_sha
    ):
        raise RuntimeError("invalid companion identity")
    if not re.fullmatch("[0-9a-f]{48}", password):
        raise RuntimeError("invalid disposable password format")
    environment = "staging" if selected_environment == "production" else "production"
    checkout = (
        Path("/home/verdaxis-prod/verdaxis")
        / ("prod" if environment == "production" else "staging")
        / "be"
    )
    suffix = "_staging" if environment == "staging" else ""
    unit_suffix = "-staging" if environment == "staging" else ""
    database = "verdaxis_staging" if environment == "staging" else "verdaxis_prod"
    app, migrator, backup = [
        name + suffix
        for name in ("verdaxis_app", "verdaxis_migrator", "verdaxis_backup")
    ]
    checkout.parent.mkdir(parents=True)
    _run("git", "-c", f"safe.directory={root}", "clone", "--no-local", root, checkout)
    _run(
        "git",
        "-C",
        checkout,
        "checkout",
        "-B",
        "staging" if environment == "staging" else "prod",
        old_sha,
    )
    for role in (app, migrator, backup):
        _sql("postgres", f"CREATE ROLE {role} LOGIN NOINHERIT PASSWORD '{password}';")
    _sql("postgres", f"CREATE DATABASE {database} OWNER {migrator};")
    _sql(
        database, f"CREATE EXTENSION postgis; ALTER SCHEMA public OWNER TO {migrator};"
    )
    app_url = f"postgresql://{app}:{password}@127.0.0.1:5432/{database}"
    migration_url = f"postgresql://{migrator}:{password}@127.0.0.1:5432/{database}"
    jwt_secret = secrets.token_hex(32)
    migration_environment = os.environ | {
        "ENVIRONMENT": "test",
        "JWT_SECRET": jwt_secret,
        "DATABASE_URL": migration_url.replace("postgresql:", "postgresql+asyncpg:"),
        "MIGRATOR_DATABASE_URL": migration_url.replace(
            "postgresql:", "postgresql+asyncpg:"
        ),
    }
    _run(
        root / "venv/bin/python",
        "-m",
        "alembic",
        "upgrade",
        "ob_20261031_offboarding",
        env=migration_environment,
        cwd=root,
    )
    _run(
        "runuser",
        "-u",
        "postgres",
        "--",
        "psql",
        "-X",
        "-v",
        "ON_ERROR_STOP=1",
        "-d",
        database,
        "-v",
        f"database_name={database}",
        "-v",
        f"app_role={app}",
        "-v",
        f"migrator_role={migrator}",
        "-v",
        f"backup_role={backup}",
        "--single-transaction",
        "-f",
        root / "deploy/postgres/converge_runtime_object_acls.sql",
    )
    if (
        _sql(database, "SELECT version_num FROM alembic_version")
        != "ob_20261031_offboarding"
    ):
        raise RuntimeError("companion revision differs")
    shutil.copytree(root / "venv", checkout / "venv", symlinks=False)
    for path in (checkout / "venv").rglob("*"):
        if path.is_file():
            path.chmod(0o700 if os.access(path, os.X_OK) else 0o600)
    domain = (
        "api-staging.verdaxis.exchange"
        if environment == "staging"
        else "api.verdaxis.exchange"
    )
    _write(
        checkout / ".env",
        f"ENVIRONMENT={environment}\nRELEASE_SHA={old_sha}\nDATABASE_HOST=127.0.0.1\nDATABASE_NAME={database}\nDATABASE_USER={app}\nDATABASE_PASSWORD={password}\nDATABASE_URL={app_url.replace('postgresql:', 'postgresql+asyncpg:')}\nMIGRATOR_DATABASE_URL={migration_url.replace('postgresql:', 'postgresql+asyncpg:')}\nJWT_SECRET={jwt_secret}\nJWT_ISSUER=verdaxis-rehearsal\nJWT_AUDIENCE=verdaxis-rehearsal\nFRONTEND_URL=https://{domain}\nEMAIL_FROM=rehearsal@example.invalid\nRESEND_API_KEY=disposable-never-send\nKYC_EXECUTION_GATE_ENABLED=false\nRUST_CACHE_LISTENER=0\nDB_SERVICE_COUNT=2\nUVICORN_WORKERS=4\nDB_POOL_SIZE=2\nDB_MAX_OVERFLOW=1\nDB_RESERVED_CONNECTIONS=20\nDB_MAX_CONNECTIONS=100\n",
    )
    (checkout / ".runtime-rust").mkdir(mode=0o700)
    (checkout / ".runtime-deploy").mkdir(mode=0o700)
    for tail in ("lock", "restart.lock"):
        _write(checkout / f".runtime-deploy/{environment}.{tail}", "")
    shutil.copyfile(previous_binary_path, checkout / ".runtime-rust/verdaxis-read")
    (checkout / ".runtime-rust/verdaxis-read").chmod(0o755)
    _run(
        root / "venv/bin/python",
        checkout / "scripts/prepare_rust_budget.py",
        "--environment-file",
        checkout / ".env",
        "--release-sha",
        old_sha,
        "--stream-listener",
        "1",
        "--output",
        checkout / ".runtime-rust/budget.json",
        cwd=checkout,
    )
    _run(
        root / "venv/bin/python",
        checkout / "scripts/prepare_rust_runtime.py",
        "--environment-file",
        checkout / ".env",
        "--release-sha",
        old_sha,
        "--environment",
        environment,
        "--source-root",
        checkout,
        "--migration-revision",
        "ob_20261031_offboarding",
        "--output-dir",
        checkout / ".runtime-rust",
        cwd=checkout,
    )
    _write(
        checkout / ".runtime-release.env",
        f"ENVIRONMENT={environment}\nRELEASE_SHA={old_sha}\nMIGRATION_REVISION=ob_20261031_offboarding\n",
    )
    _write(
        checkout / ".runtime-monitor.env",
        f"ENVIRONMENT={environment}\nRELEASE_SHA={old_sha}\n",
    )
    _write(
        checkout / ".runtime-rust/revision.json",
        {"release_sha": old_sha, "revision": "ob_20261031_offboarding"},
    )
    shutil.copyfile(
        checkout / f"deploy/rust/{environment}-policy.json",
        checkout / f".runtime-rust/{environment}-policy.json",
    )
    _write(f"/etc/verdaxis/kyc-document-keys{unit_suffix}.json", "", 0o400)
    _run("chown", "-R", "verdaxis-prod:verdaxis-prod", checkout)
    _run("chown", "verdaxis-prod:verdaxis-prod", checkout.parent)
    _run("chmod", "0750", checkout.parent, checkout)
    _run("setfacl", "-m", "u:caddy:--x", checkout.parent, checkout)
    unit = f"verdaxis-rust{unit_suffix}.service"
    _run("systemctl", "daemon-reload")
    _run("systemctl", "start", unit)
    observed = _ready(environment, 8011 if environment == "staging" else 8010, old_sha)
    old_python_backends = _old_python_backends(old_sha)
    aliases = _aliases(old_sha)
    monitors = _monitors(checkout, old_sha)
    backups = _backups()
    schedules = _preserved_schedules()
    launchers = {
        "news": [str(current_checkout / "app/cli/refresh_news.py")],
        "auth-prune": [
            str(current_checkout / "app/cli/auth_maintenance.py"),
            str(current_checkout / "scripts/prune_product_analytics.py"),
        ],
        "reminders": [str(current_checkout / "app/cli/order_expiry_reminders.py")],
        "onboarding": [str(current_checkout / "app/cli/onboarding_attention.py")],
        "demo": ["/usr/local/libexec/verdaxis-monitor/verify_runtime_identity.py"],
        "local-monitor": [
            "/usr/local/libexec/verdaxis-monitor/local_health_check.py",
            "/usr/local/libexec/verdaxis-monitor/backup_verify.py",
        ],
        "external-recovery": [
            "/usr/local/sbin/verdaxis-monitor",
            "/usr/local/sbin/verdaxis-recover",
            "/usr/local/sbin/verdaxis-autodiag",
        ],
    }
    if not all(Path(path).is_file() for paths in launchers.values() for path in paths):
        raise RuntimeError("a real retained direct launcher is missing")
    return {
        "launchers": launchers,
        "environment": environment,
        "checkout": str(checkout),
        "database": database,
        "rust_unit": unit,
        "ready": observed,
        "alias_units": aliases,
        "old_python_backends": old_python_backends,
        "monitor_units": monitors,
        "backups": backups,
        "schedules": schedules,
        "runtime_package_roots": [str(checkout / "venv")],
        "package_roots": ["/usr/local/libexec/verdaxis-monitor"],
        "monitor_root": "/usr/local/libexec/verdaxis-monitor",
        "monitor_binary": "/usr/local/libexec/verdaxis-monitor/local_health_check.py",
        "selected_timers": sorted(
            path.name
            for path in (current_checkout / "deploy/systemd").glob("*.timer")
            if ("staging" in path.name) == (selected_environment == "staging")
        ),
    }
