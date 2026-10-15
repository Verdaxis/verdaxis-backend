"""Own a pinned disposable cluster; never accept a caller database URL."""

from __future__ import annotations

import hashlib
import json
import os
import xml.etree.ElementTree as ET
import signal
import subprocess
import sys
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272"
PINS = {
    "prod": "c08266bd6969143812dec7c5033338428cd5f159",
    "staging": "b218ab264b92f99007bacc05d4871413e024b72a",
}


# The fifth Review Focus case runs in the frontend repository. Its immutable
# two-bundle evidence and accepted exception are signed in acceptance.md.
REQUIRED_TESTS = {
    "test_historical_snapshot_import_restores_exact_guards",
    "test_copy_cycles_and_next_sequence_values",
    "test_interrupt_each_cutover_boundary_keeps_runtime_tuple_blocked",
    "test_timezone_reconnect_and_role_override",
    "test_bootstrap_uses_only_checked_connection",
    "test_outbox_drain_before_final_snapshot",
}


def default_test_paths():
    paths = list((ROOT / "tests/phase0").glob("test_*.py"))
    paths.extend((ROOT / "tests/postgres").glob("test_phase0_*.py"))
    return sorted(str(path.relative_to(ROOT)) for path in paths)


def check_test_report(path, required_suites, required_tests):
    cases = ET.parse(path).getroot().findall(".//testcase")
    failed = sum(case.find("failure") is not None or case.find("error") is not None for case in cases)
    skipped = sum(case.find("skipped") is not None for case in cases)
    if not cases or failed or skipped:
        raise ValueError("Required run has zero tests, failures or skips")
    suites = {case.get("classname", "") for case in cases}
    names = {case.get("name", "").split("[", 1)[0] for case in cases}
    missing_suites = required_suites - suites
    missing_tests = required_tests - names
    if missing_suites or missing_tests:
        raise ValueError(f"Missing required tests: {sorted(missing_suites | missing_tests)}")
    return {"passed": len(cases), "failed": failed, "skipped": skipped,
            "suites": sorted(suites), "required_tests": sorted(required_tests)}


def command(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, **kwargs)


def main():
    for argument in sys.argv[1:]:
        if "://" in argument:
            raise SystemExit("Database URLs are not accepted")
    container = f"verdaxis-phase0-{uuid.uuid4().hex[:12]}"
    password = uuid.uuid4().hex
    directory = ROOT / "work/phase0-fixtures" / uuid.uuid4().hex
    directory.mkdir(parents=True)
    # Only explicit test settings reach pinned code. No provider keys or .env.
    environment = {
        "PATH": os.environ["PATH"], "HOME": str(directory),
        "ENVIRONMENT": "test", "RELEASE_SHA": "test",
        "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
        "PYTHONDONTWRITEBYTECODE": "1", "KYC_EXECUTION_GATE_ENABLED": "false",
    }
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    try:
        command("docker", "run", "-d", "--platform", "linux/amd64", "--name", container,
                "-e", f"POSTGRES_PASSWORD={password}",
                "-p", "127.0.0.1::5432", IMAGE, stdout=subprocess.DEVNULL)
        # The pinned amd64 image initializes under emulation and can take over a
        # minute on a loaded host, so allow a generous readiness window.
        for attempt in range(240):
            ready = subprocess.run(["docker", "exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], capture_output=True)
            if ready.returncode == 0:
                break
            time.sleep(1)
        else:
            raise RuntimeError("Disposable PostgreSQL did not become ready")
        port = command("docker", "port", container, "5432/tcp", capture_output=True).stdout.strip().split(":")[-1]
        environment["PHASE0_PORT"] = port
        environment["PHASE0_CLUSTER_ID"] = command(
            "docker", "exec", container, "psql", "-X", "-U", "postgres", "-Atc",
            "SELECT system_identifier FROM pg_control_system()", capture_output=True,
        ).stdout.strip()
        environment["PHASE0_PASSWORD"] = password
        environment["PHASE0_WORK"] = str(directory)
        guard = directory / "network-guard"
        guard.mkdir()
        (guard / "sitecustomize.py").write_bytes((ROOT / "tests/phase0/network_guard.py").read_bytes())
        environment["PYTHONPATH"] = str(guard)
        command("docker", "exec", container, "psql", "-X", "-U", "postgres", "-v", "ON_ERROR_STOP=1", "-c",
                f"CREATE ROLE phase0_owner LOGIN NOINHERIT PASSWORD '{password}'")
        command("docker", "exec", container, "psql", "-X", "-U", "postgres", "-d", "template1", "-v", "ON_ERROR_STOP=1", "-c",
                "CREATE EXTENSION IF NOT EXISTS postgis")
        for name, sha in PINS.items():
            tree = directory / name
            tree.mkdir()
            archive = directory / f"{name}.tar"
            with archive.open("wb") as output:
                subprocess.run(["git", "archive", sha], cwd=ROOT, stdout=output, check=True)
            command("tar", "-xf", str(archive), "-C", str(tree))
            # Reuse installed packages without modifying the shared environment.
            (tree / "venv").symlink_to(ROOT / "venv", target_is_directory=True)
            database = f"phase0_{name}_test"
            command("docker", "exec", container, "createdb", "-U", "postgres", "-O", "phase0_owner", database)
            url = f"postgresql+asyncpg://phase0_owner:{password}@127.0.0.1:{port}/{database}"
            migration_env = dict(environment, DATABASE_URL=url, MIGRATOR_DATABASE_URL=url)
            revision = "miq_20260720_market_quarantine" if name == "prod" else "kycd_20261015_staging"
            command(str(tree / "venv/bin/python"), "-m", "alembic", "upgrade", revision,
                    cwd=tree, env=migration_env)
        command(str(ROOT / "venv/bin/python"), "-m", "tests.phase0.build_fixtures", cwd=ROOT, env=environment)
        if any('test_runtime_role_policy' in argument for argument in sys.argv[1:]):
            # Reuse the existing raw-role suite without its broader seed harness
            # or any replication-role grant. Only this disposable cluster changes.
            database = 'phase0_roles_analytics_test'
            command('docker', 'exec', container, 'createdb', '-U', 'postgres',
                    '-T', 'phase0_staging_test', database)
            policy = '/tmp/phase0-role-policy'
            command('docker', 'exec', container, 'mkdir', '-p', policy)
            for filename in ('app_acl_policy.sql', 'converge_runtime_object_acls.sql', 'bootstrap_roles.sql', 'validate_roles.sql'):
                command('docker', 'cp', str(ROOT / 'deploy/postgres' / filename), f'{container}:{policy}/{filename}')
            roles = {'app_role': 'phase0_app_test', 'migrator_role': 'phase0_migrator_test', 'backup_role': 'phase0_backup_test'}
            variables = ['-v', f'database_name={database}']
            for key, role in roles.items():
                variables.extend(['-v', f'{key}={role}'])
            command('docker', 'exec', container, 'psql', '-X', '-U', 'postgres', '-d', database,
                    *variables, '-f', f'{policy}/bootstrap_roles.sql')
            for role in roles.values():
                command('docker', 'exec', container, 'psql', '-X', '-U', 'postgres', '-c',
                        f"ALTER ROLE {role} PASSWORD '{password}'")
            for key, role in roles.items():
                environment['RUNTIME_TEST_' + key.upper()] = role
            environment['RUNTIME_TEST_DATABASE_NAME'] = database
            for key, role in {'DATABASE_URL': roles['app_role'], 'MIGRATOR_DATABASE_URL': roles['migrator_role'],
                              'PRODUCT_ANALYTICS_TEST_DATABASE_URL': roles['migrator_role'],
                              'BACKUP_DATABASE_URL': roles['backup_role'], 'POSTGRES_ADMIN_TEST_DATABASE_URL': 'postgres'}.items():
                environment[key] = f'postgresql+asyncpg://{role}:{password}@127.0.0.1:{port}/{database}'
        print(f"Fixture evidence: {directory}", flush=True)
        full_run = not sys.argv[1:]
        paths = default_test_paths() if full_run else sys.argv[1:]
        report = directory / "pytest.xml"
        invocation = [str(ROOT / "venv/bin/python"), "-m", "pytest", *paths,
                      "-q", f"--junitxml={report}"]
        result = subprocess.run(invocation, cwd=ROOT, env=environment)
        evidence = {"command": invocation, "returncode": result.returncode,
                    "backend_sha": command("git", "rev-parse", "HEAD", cwd=ROOT, capture_output=True).stdout.strip(),
                    "complete_phase0_run": full_run, "image": IMAGE}
        if result.returncode == 0:
            required = {str(Path(path).with_suffix("")).replace("/", ".") for path in default_test_paths()} if full_run else set()
            evidence["tests"] = check_test_report(report, required, REQUIRED_TESTS if full_run else set())
        evidence["sha256"] = {
            str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in [ROOT / "deploy/phase0/copy-manifest.json",
                         ROOT / "deploy/phase0/bootstrap-policy.json",
                         ROOT / "deploy/migration-checkpoints.tsv", report]
        }
        (directory / "runner-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
        return result.returncode
    finally:
        command("docker", "rm", "-f", "-v", container, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    sys.exit(main())
