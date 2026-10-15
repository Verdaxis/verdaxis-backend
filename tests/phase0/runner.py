"""Own a pinned disposable cluster; never accept a caller database URL."""

from __future__ import annotations

import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "postgis/postgis:17-3.6-alpine@sha256:49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272"
PINS = {
    "prod": "c08266bd6969143812dec7c5033338428cd5f159",
    "staging": "b218ab264b92f99007bacc05d4871413e024b72a",
}


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
        for attempt in range(60):
            ready = subprocess.run(["docker", "exec", container, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], capture_output=True)
            if ready.returncode == 0:
                break
            time.sleep(1)
        else:
            raise RuntimeError("Disposable PostgreSQL did not become ready")
        port = command("docker", "port", container, "5432/tcp", capture_output=True).stdout.strip().split(":")[-1]
        environment["PHASE0_PORT"] = port
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
        print(f"Fixture evidence: {directory}", flush=True)
        result = subprocess.run([str(ROOT / "venv/bin/python"), "-m", "pytest", *(sys.argv[1:] or ["tests/phase0"]), "-q"], cwd=ROOT, env=environment)
        return result.returncode
    finally:
        command("docker", "rm", "-f", "-v", container, stdout=subprocess.DEVNULL)


if __name__ == "__main__":
    sys.exit(main())
