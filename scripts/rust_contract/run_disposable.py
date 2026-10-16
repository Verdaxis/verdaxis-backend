"""One disposable runner: database, Python oracle, Rust server, comparator.

It starts a disposable PostgreSQL container on a Docker-assigned loopback port
with the pinned PostGIS digest, migrates it, optionally seeds it, starts the
Python oracle wrapper and the Rust contract server on ephemeral loopback ports,
attests both, then runs one Rust contract target through the authoritative
Python comparator. It removes only its own container and children in a finally
block. Never point this at staging or production.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
IMAGE = (
    "postgis/postgis:17-3.6-alpine@sha256:"
    "49b4d46c9fb8b158ddecd14d1894871a21ed4acb7ef376e2050f43f79c7b7272"
)
DB_NAME = "verdaxis_rust_test"
APP_ROLE = "rust_owner"
APP_PASSWORD = "disposable_owner_pw"
MIGRATION_REVISION = "kycg_20261014_staging"
# The staging oracle's session time zone. Recorded with each golden run.
ORACLE_TIMEZONE = "Asia/Singapore"
# One explicit instant for the fixed-clock corpus. Every server sees it.
CORPUS_CLOCK = "2026-10-15T12:00:00+00:00"

# group -> (rust test target, route module set). An unavailable route set fails.
GROUPS = {
    "health": "health",
    "echo": "echo",
    "simple_reads": "simple_reads",
    "live_benchmarks": "live_benchmarks",
    "orderbook_snapshot": "orderbook_snapshot",
    "orderbook_lists": "orderbook_lists",
    "orderbook_map": "orderbook_map",
    "prices": "prices",
    "curves_legacy": "curves_legacy",
    "curves_canonical": "curves_canonical",
    "mcp": "mcp",
}
IDENTITY_PATH = "/.well-known/verdaxis-disposable-test"


class RunnerError(RuntimeError):
    """A disposable-run step failed."""


def ephemeral_range() -> tuple[int, int]:
    from tests.ephemeral_ports import EphemeralRangeError, ephemeral_range as kernel_range
    try:
        return kernel_range()
    except EphemeralRangeError as error:
        raise RunnerError(str(error)) from error


def ephemeral_port() -> int:
    first, last = ephemeral_range()
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    if not first <= port <= last:
        raise RunnerError(f"kernel returned a non-disposable port {port}")
    return port



def docker(*args: str, check: bool = True, input_text: str | None = None) -> str:
    result = subprocess.run(
        ["docker", *args], capture_output=True, text=True, input=input_text
    )
    if check and result.returncode != 0:
        raise RunnerError(f"docker {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def wait_for_port(port: int, timeout: float = 60.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        with socket.socket() as probe:
            if probe.connect_ex(("127.0.0.1", port)) == 0:
                return
        time.sleep(0.2)
    raise RunnerError(f"nothing accepted a connection on 127.0.0.1:{port}")


def attest(base_url: str, token: str) -> None:
    request = urllib.request.Request(base_url + IDENTITY_PATH, method="GET")
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            payload = json.loads(response.read())
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise RunnerError(f"handshake failed for {base_url}") from error
    if payload.get("disposable") is not True or payload.get("token") != token:
        raise RunnerError(f"{base_url} is not the attested disposable target")


class Runner:
    def __init__(
        self,
        group: str,
        *,
        seed: bool,
        keep: bool,
        seed_mode: str,
        record_all: bool = False,
        gate: bool = True,
        mixed_caddy: bool = False,
    ) -> None:
        self.mixed_caddy = mixed_caddy
        self.record_all = record_all
        self.gate = gate
        self.group = group
        self.seed = seed
        self.keep = keep
        self.seed_mode = seed_mode
        self.container = ""
        self.children: list[subprocess.Popen] = []
        self.sha = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
        self.token = secrets.token_urlsafe(32)
        self.python_bin = str(ROOT / "venv" / "bin" / "python")

    def start_database(self) -> int:
        self.container = docker(
            "run",
            "--platform",
            "linux/amd64",
            "-d",
            "--rm",
            "-e",
            "POSTGRES_PASSWORD=disposable_admin_pw",
            "-p",
            "127.0.0.1::5432",
            IMAGE,
            "-c",
            "shared_preload_libraries=pg_stat_statements",
        ).strip()
        # pg_isready prints its status to stdout even when it fails, so the
        # exit status, not the output, decides readiness.
        for _ in range(90):
            probe = subprocess.run(
                [
                    "docker",
                    "exec",
                    self.container,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "postgres",
                ],
                capture_output=True,
                text=True,
            )
            if probe.returncode == 0:
                break
            time.sleep(1)
        else:
            raise RunnerError("the disposable database never became ready")
        port = docker("port", self.container, "5432/tcp").strip().rsplit(":", 1)[-1]
        docker(
            "exec",
            "-i",
            self.container,
            "psql",
            "-q",
            "-U",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
            input_text=f"""
CREATE ROLE {APP_ROLE} LOGIN NOINHERIT NOSUPERUSER PASSWORD '{APP_PASSWORD}';
CREATE DATABASE {DB_NAME} OWNER {APP_ROLE};
\\connect {DB_NAME}
CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
ALTER DATABASE {DB_NAME} SET TimeZone TO '{ORACLE_TIMEZONE}';
\\connect template1
CREATE EXTENSION IF NOT EXISTS postgis;
""",
        )
        return int(port)

    def urls(self, port: int) -> dict[str, str]:
        base = f"{APP_ROLE}:{APP_PASSWORD}@127.0.0.1:{port}/{DB_NAME}"
        return {
            "DATABASE_URL": f"postgresql+asyncpg://{base}",
            "MIGRATOR_DATABASE_URL": f"postgresql+asyncpg://{base}",
            "SEED_DATABASE_URL": f"postgresql://{base}",
            "SEED_TARGET_DATABASE": DB_NAME,
            "SEED_RUNTIME_ENV": "disposable",
            "ALLOW_SEED_MUTATIONS": "I_UNDERSTAND_SEED_MUTATIONS",
            "RUST_DATABASE_URL": f"postgres://{base}",
        }

    def migrate_and_seed(self, env: dict[str, str]) -> None:
        run = {**os.environ, **env}
        alembic = [str(ROOT / "venv/bin/alembic"), "upgrade"]
        if self.seed and self.seed_mode == "corpus":
            subprocess.run(
                alembic + ["miq_20260720_market_quarantine"],
                cwd=ROOT,
                env=run,
                check=True,
            )
            subprocess.run(
                [self.python_bin, "-m", "scripts.rust_contract.seed_corpus"],
                cwd=ROOT,
                env={**run, "RUST_CORPUS_STAGE": "legacy"},
                check=True,
            )
        # The shared verifier owns the head upgrade: it refuses anything but one
        # standalone Alembic head before upgrading, then checks the heads and
        # model/schema drift. A bare `upgrade head` here skips all three checks.
        subprocess.run(
            [str(ROOT / "scripts" / "verify_migrations.sh")],
            cwd=ROOT,
            env={**run, "ALEMBIC_BIN": str(ROOT / "venv" / "bin" / "alembic")},
            check=True,
        )
        if not self.seed:
            return
        if self.seed_mode == "corpus":
            subprocess.run(
                [self.python_bin, "-m", "scripts.rust_contract.seed_corpus"],
                cwd=ROOT,
                env=run,
                check=True,
            )
        else:
            subprocess.run(
                [self.python_bin, "scripts/seed.py"],
                cwd=ROOT,
                env={**run, "PYTHONPATH": str(ROOT)},
                check=True,
            )
            print("seed mode ordinary: corpus fixture families are not applied")

    def spawn(self, args: list[str], env: dict[str, str]) -> None:
        self.children.append(
            subprocess.Popen(
                args, cwd=ROOT, env=env, stdout=sys.stdout, stderr=sys.stderr
            )
        )

    def run(self) -> None:
        subprocess.run(
            [
                "cargo",
                "build",
                "--manifest-path",
                "rs/Cargo.toml",
                "--locked",
                "--features",
                "contract-tests",
                "--example",
                "contract_server",
            ],
            cwd=ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha},
            check=True,
        )
        port = self.start_database()
        base_env = {
            **os.environ,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": self.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": self.token,
            "RUST_CONTRACT_CLOCK": CORPUS_CLOCK,
            "KYC_EXECUTION_GATE_ENABLED": str(self.gate).lower(),
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **self.urls(port),
        }
        self.migrate_and_seed(base_env)

        python_port = ephemeral_port()
        rust_port = ephemeral_port()
        self.spawn(
            [
                self.python_bin,
                "-m",
                "tests.rust_contract.server",
                "--port",
                str(python_port),
            ],
            {**base_env, "PYTHONPATH": str(ROOT)},
        )
        if self.record_all:
            wait_for_port(python_port)
            python_url = f"http://127.0.0.1:{python_port}"
            from unittest.mock import patch

            with patch.dict(os.environ, base_env):
                self.record_groups(python_url)
            return
        rust_config = json.dumps(
            {
                "environment": "test",
                "kyc_execution_gate_enabled": self.gate,
                "release_sha": self.sha,
                "migration_revision": MIGRATION_REVISION,
                "database_url": base_env["RUST_DATABASE_URL"],
                "db_session_time_zone": ORACLE_TIMEZONE,
                "bind": f"127.0.0.1:{rust_port}",
                "pool_max": 9,
                "python_budget": 52,
                "configured_max_connections": 100,
            }
        )
        self.spawn(
            [str(ROOT / "rs" / "target" / "debug" / "examples" / "contract_server")],
            {**base_env, "RUST_CONFIG": rust_config, "CONTRACT_GROUP": self.group},
        )
        wait_for_port(python_port)
        wait_for_port(rust_port)
        python_url = f"http://127.0.0.1:{python_port}"
        rust_url = f"http://127.0.0.1:{rust_port}"
        attest(python_url, self.token)
        attest(rust_url, self.token)

        if self.mixed_caddy:
            self.run_mixed(base_env, python_port, rust_port)
            return

        target = GROUPS[self.group]
        if not (ROOT / "rs/contract/tests" / f"{target}.rs").exists():
            # Unported groups still have a router (currently empty). Compare it
            # honestly instead of failing before any HTTP request is sent.
            subprocess.run(
                [self.python_bin, "-m", "scripts.rust_contract.compare", "--group", self.group],
                cwd=ROOT,
                env={**base_env, "PYTHON_CONTRACT_URL": python_url, "RUST_CONTRACT_URL": rust_url},
                check=True,
            )
            return
        subprocess.run(
            [
                "cargo",
                "test",
                "--manifest-path",
                "rs/Cargo.toml",
                "--locked",
                "-p",
                "verdaxis-contract",
                "--features",
                "contract-http",
                "--test",
                target,
            ],
            cwd=ROOT,
            env={
                **base_env,
                "PYTHON_CONTRACT_URL": python_url,
                "RUST_CONTRACT_URL": rust_url,
            },
            check=True,
        )

    def run_mixed(self, env: dict[str, str], python_port: int, rust_port: int) -> None:
        caddy = os.environ.get("CADDY_BIN") or shutil.which("caddy")
        if not caddy:
            caddy = str(ROOT / "work/rust-phase1/bin/caddy")
        if not Path(caddy).is_file():
            raise RunnerError("Set CADDY_BIN to a local Caddy executable")
        port = ephemeral_port()
        caddy_env = {
            **env,
            "ROUTE_SNIPPET": str(ROOT / "deploy/caddy/rust-public-staging.caddy"),
            "CADDY_PORT": str(port),
            "PYTHON_UPSTREAM": f"127.0.0.1:{python_port}",
            **{f"{group}_UPSTREAM": f"127.0.0.1:{rust_port}"
               for group in ("SIMPLE", "ORDERBOOK", "PRICES", "CURVES", "MCP")},
        }
        config = str(ROOT / "tests/rust_contract/Caddyfile")
        subprocess.run([caddy, "adapt", "--config", config, "--validate"],
                       cwd=ROOT, env=caddy_env, check=True)
        self.spawn([caddy, "run", "--config", config], caddy_env)
        wait_for_port(port)
        url = f"http://127.0.0.1:{port}"
        attest(url, self.token)
        subprocess.run(
            [self.python_bin, "-m", "pytest", "tests/integration", "-q",
             "--run-disposable-integration", "--disposable-target-url", url,
             "--disposable-target-token", self.token],
            cwd=ROOT,
            env={**env, "DISPOSABLE_ITEST_PASSWORD": "Disposable-integration-13!"},
            check=True,
        )

    def record_groups(self, python_url: str) -> None:
        sys.path.insert(0, str(ROOT))
        from scripts.rust_contract.record import differences, record, write_golden

        cases = json.loads((ROOT / "tests/rust_contract/cases.json").read_text())[
            "groups"
        ]
        fixtures = json.loads(
            (ROOT / "work/rust-phase1/task4b-fixtures.json").read_text()
        )
        from scripts.rust_contract.compare import ContractError
        from scripts.rust_contract.record import stable

        unstable = []
        recordings = []
        failures = []
        evidence = (
            ROOT
            / "work/rust-phase1"
            / ("record-enabled" if self.gate else "record-disabled")
        )
        evidence.mkdir(parents=True, exist_ok=True)
        import signal

        def deadline(signum, frame):
            raise TimeoutError("recording deadline exceeded (stream did not close)")

        for group, requests in cases.items():
            if group == "mcp":
                print("RECORD_EXCLUDED mcp: decision 19; Task 11", flush=True)
                continue
            previous = signal.signal(signal.SIGALRM, deadline)
            try:
                signal.alarm(90)
                first = record(python_url, self.token, requests)
                second = record(python_url, self.token, requests)
            except (ContractError, TimeoutError) as error:
                failures.append(f"{group}: {error}")
                print(f"RECORD_FAILED {group}: {error}", flush=True)
                continue
            finally:
                signal.alarm(0)
                signal.signal(signal.SIGALRM, previous)
            problems = differences(first, second)
            unstable.extend(problems)
            print(
                f"RECORD {group}: {len(first)} cases; unstable={len(problems)}",
                flush=True,
            )
            for problem in problems:
                print(problem, flush=True)
            for label, entries in (("first", first), ("second", second)):
                (evidence / f"{group}-{label}.json").write_text(
                    json.dumps(stable(entries), indent=2, default=str) + "\n"
                )
            recordings.append((group, first))
        print(
            f"TOTAL_UNSTABLE={len(unstable)}; RECORD_FAILURES={len(failures)}",
            flush=True,
        )
        (evidence / "summary.json").write_text(
            json.dumps(
                {
                    "source_sha": self.sha,
                    "KYC_EXECUTION_GATE_ENABLED": self.gate,
                    "unstable": unstable,
                    "failures": failures,
                },
                indent=2,
            )
            + "\n"
        )
        if unstable or failures:
            raise RunnerError(
                "oracle recording is incomplete or unstable; golden publication refused"
            )
        for group, entries in recordings:
            if not self.gate:
                entries = [
                    {**entry, "id": entry["id"] + "--kyc-off"} for entry in entries
                ]
            write_golden(
                ROOT / "tests/rust_contract",
                self.sha,
                entries,
                meta={
                    "source_sha": self.sha,
                    "schema_revision": "kycd_20261015_staging",
                    "timezone": ORACLE_TIMEZONE,
                    "clock": CORPUS_CLOCK,
                    "group": group,
                    "KYC_EXECUTION_GATE_ENABLED": self.gate,
                    "seed_mode": "corpus",
                    "fixture_counts": fixtures["counts"],
                },
            )

    def cleanup(self) -> None:
        for child in self.children:
            child.terminate()
        for child in self.children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
        if self.container and not self.keep:
            # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
            docker("rm", "-f", "-v", self.container, check=False)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--group", default="health", choices=sorted([*GROUPS, "all"]))
    parser.add_argument("--mixed-caddy", action="store_true")
    parser.add_argument("--record-all", action="store_true")
    parser.add_argument(
        "--kyc-gate", choices=("enabled", "disabled"), default="enabled"
    )
    parser.add_argument(
        "--no-seed",
        action="store_true",
        help="skip the ordinary seed; only valid for groups with no data cases",
    )
    parser.add_argument(
        "--keep", action="store_true", help="leave the container behind"
    )
    parser.add_argument(
        "--seed-mode",
        choices=("corpus", "ordinary"),
        default="corpus",
        help="ordinary applies only scripts/seed.py; corpus also applies the fixture families",
    )
    args = parser.parse_args(argv)
    if (args.group == "all") != args.mixed_caddy or (args.mixed_caddy and args.record_all):
        parser.error("--mixed-caddy requires --group all and cannot record golden data")
    if args.no_seed and args.group not in ("health", "echo"):
        print("--no-seed is only valid for the health and echo groups", file=sys.stderr)
        return 2
    if shutil.which("docker") is None:
        print("docker is required", file=sys.stderr)
        return 2

    runner = Runner(
        args.group,
        seed=not args.no_seed,
        keep=args.keep,
        seed_mode=args.seed_mode,
        record_all=args.record_all,
        gate=args.kyc_gate == "enabled",
        mixed_caddy=args.mixed_caddy,
    )
    try:
        runner.run()
    except (RunnerError, subprocess.CalledProcessError) as error:
        print(f"disposable run failed: {error}", file=sys.stderr)
        return 1
    finally:
        runner.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
