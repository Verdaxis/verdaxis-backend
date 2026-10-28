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
import math
import os
import re
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
# Support both `python -m` and the documented direct script invocation.
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
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
# Password for the integration suite's deterministic itest identities. The
# seeder and pytest must use the same value, or every login returns 401.
ITEST_PASSWORD = "Disposable-integration-13!"

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


class OracleStartupUnavailable(RunnerError):
    """No Python listener after the bounded startup attempts."""


def ephemeral_range() -> tuple[int, int]:
    from tests.ephemeral_ports import EphemeralRangeError
    from tests.ephemeral_ports import ephemeral_range as kernel_range
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
        rust_target: str | None = None,
        require_complete_ledger: bool = False,
        report: Path | None = None,
        release: bool = False,
        server_binary: Path | None = None,
    ) -> None:
        self.mixed_caddy = mixed_caddy
        self.record_all = record_all
        self.gate = gate
        self.group = group
        self.seed = seed
        self.keep = keep
        self.seed_mode = seed_mode
        # Correctness runs stay on the dev profile (it panics on integer
        # overflow and so catches more defects). Latency measurements must use
        # the release profile, or they time unoptimized code. PM decision.
        self.release = release
        self.server_binary = server_binary
        # Which Rust contract targets to run. A single group defaults to its own
        # target; `--group all` alone runs none (integration only), while
        # `--rust-target all` runs every target once in this shared database.
        self.rust_target = rust_target or (group if group != "all" else None)
        self.require_complete_ledger = require_complete_ledger
        self.report = report
        self.container = ""
        self.children: list[subprocess.Popen] = []
        self.oracle_retries: list[dict] = []
        self.oracle_unavailable_groups: list[str] = []
        self.target_results: dict[str, bool] = {}
        # Per attempt; bounded to one initial start and at most two restarts.
        try:
            self.oracle_startup_timeout = float(os.environ.get("ORACLE_STARTUP_TIMEOUT_SECONDS", "180"))
        except ValueError as error:
            raise RunnerError("ORACLE_STARTUP_TIMEOUT_SECONDS must be positive and finite") from error
        if not math.isfinite(self.oracle_startup_timeout) or self.oracle_startup_timeout <= 0:
            raise RunnerError("ORACLE_STARTUP_TIMEOUT_SECONDS must be positive and finite")
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

    def build_command(self) -> list[str]:
        """The cargo build for this run's Rust contract server.

        Dev profile for correctness runs (overflow panics catch defects);
        `--release` only when a measurement run asked for it explicitly.
        """
        return [
            "cargo",
            "build",
            "--manifest-path",
            "rs/Cargo.toml",
            "--locked",
            "--features",
            "contract-tests",
            "--example",
            "contract_server",
            *(["--release"] if self.release else []),
        ]

    def server_path(self) -> Path:
        """The contract-server binary this run's build produced."""
        if self.server_binary is not None:
            return self.server_binary
        flavour = "release" if self.release else "debug"
        target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "rs" / "target"))
        if not target.is_absolute():
            target = ROOT / target
        return target / flavour / "examples" / "contract_server"

    def run(self) -> None:
        try:
            self._run()
        except OracleStartupUnavailable:
            if self.report is not None:
                passed = [group for group, ok in self.target_results.items() if ok]
                failed = [group for group, ok in self.target_results.items() if not ok]
                self.write_report(self.target_results, passed, failed, None)
            raise

    def _run(self) -> None:
        subprocess.run(
            self.build_command(),
            cwd=ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha, "RUST_TEST_PYTHON": sys.executable},
            check=True,
        )
        if self.rust_target == "all":
            # Every group's extended target probes mutate the shared corpus
            # (fixture rows with deliberately invalid terms, quantity and
            # window rewrites), and the mixed integration suite writes too.
            # Those mutations make later groups' Python oracle answers change,
            # so one fresh database per group reproduces exactly what each
            # target sees when it runs alone. The mixed suite keeps its own
            # fresh database for the same reason.
            if self.mixed_caddy:
                env = self.start_environment(self.group)
                try:
                    self.run_mixed(
                        env["base_env"], env["python_port"], env["rust_port"]
                    )
                finally:
                    self.stop_environment()
            results = self.target_results
            for group in self.target_groups():
                env = self.start_environment(group)
                try:
                    results.update(
                        self.run_targets(
                            env["base_env"],
                            env["python_url"],
                            env["rust_url"],
                            groups=[group],
                        )
                    )
                finally:
                    self.stop_environment()
            self.finalize(results)
            return
        env = self.start_environment(self.group)
        try:
            if self.record_all:
                from unittest.mock import patch

                with patch.dict(os.environ, env["base_env"]):
                    self.record_groups(env["python_url"])
                return
            if self.mixed_caddy:
                self.run_mixed(
                    env["base_env"], env["python_port"], env["rust_port"]
                )
            results = {}
            if self.rust_target:
                results = self.run_targets(
                    env["base_env"], env["python_url"], env["rust_url"]
                )
            self.finalize(results)
        finally:
            self.stop_environment()

    def start_environment(self, group: str) -> dict[str, object]:
        """Start one disposable database plus attested Python and Rust servers."""
        port = self.start_database()
        base_env = {
            **os.environ,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": self.sha,
            "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars",
            "DISPOSABLE_TEST_TOKEN": self.token,
            "RUST_CONTRACT_CLOCK": CORPUS_CLOCK,
            # Rust cases that start Python use the interpreter that runs this
            # harness, so a workflow venv and a test host agree.
            "RUST_TEST_PYTHON": sys.executable,
            "KYC_EXECUTION_GATE_ENABLED": str(self.gate).lower(),
            "RUST_CORPUS_MANIFEST": str(ROOT / "work/rust-phase1/task4b-fixtures.json"),
            **self.urls(port),
        }
        self.migrate_and_seed(base_env)

        python_port = ephemeral_port()
        rust_port = ephemeral_port()
        self.start_oracle(group, python_port, base_env)
        if self.record_all:
            return {
                "base_env": base_env,
                "python_port": python_port,
                "python_url": f"http://127.0.0.1:{python_port}",
                "rust_port": rust_port,
                "rust_url": "",
            }
        rust_config = json.dumps(
            {
                "environment": "test",
                "kyc_execution_gate_enabled": self.gate,
                "release_sha": self.sha,
                "migration_revision": MIGRATION_REVISION,
                "database_url": base_env["RUST_DATABASE_URL"],
                "db_session_time_zone": ORACLE_TIMEZONE,
                "bind": f"127.0.0.1:{rust_port}",
                "read_admission_slots": 16, "pool_max": 17,
                "python_budget": 52,
                "configured_max_connections": 100,
            }
        )
        self.spawn(
            [str(self.server_path())],
            {**base_env, "RUST_CONFIG": rust_config, "CONTRACT_GROUP": group},
        )
        wait_for_port(rust_port)
        python_url = f"http://127.0.0.1:{python_port}"
        rust_url = f"http://127.0.0.1:{rust_port}"
        attest(python_url, self.token)
        attest(rust_url, self.token)
        return {
            "base_env": base_env,
            "python_port": python_port,
            "python_url": python_url,
            "rust_port": rust_port,
            "rust_url": rust_url,
        }

    def start_oracle(self, group: str, port: int, env: dict[str, str]) -> None:
        for attempt in range(3):
            self.spawn(
                [self.python_bin, "-m", "tests.rust_contract.server", "--port", str(port)],
                {**env, "PYTHONPATH": str(ROOT)},
            )
            child = self.children[-1]
            try:
                wait_for_port(port, timeout=self.oracle_startup_timeout)
                return
            except RunnerError as error:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                self.children.remove(child)
                if attempt == 2:
                    self.oracle_unavailable_groups.append(group)
                    raise OracleStartupUnavailable(f"ORACLE_UNAVAILABLE: {group}/startup: {error}") from error
                event = {"group": group, "request": "startup", "leg": "python-startup",
                         "reason": "startup timeout", "attempt": attempt + 1,
                         "timeout_seconds": self.oracle_startup_timeout}
                self.oracle_retries.append(event)
                print("ORACLE_RETRY " + json.dumps(event), flush=True)

    def stop_environment(self) -> None:
        """Stop the servers and remove this runner's disposable container."""
        for child in self.children:
            child.terminate()
        for child in self.children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        self.children = []
        if self.container and not self.keep:
            # -v removes the anonymous data volume; a forced remove skips --rm.
            docker("rm", "-f", "-v", self.container, check=False)
        self.container = ""

    def target_groups(self) -> list[str]:
        """The groups whose Rust contract targets this run selects."""
        if self.rust_target == "all":
            return list(GROUPS)
        return [self.rust_target]

    def run_targets(
        self,
        env: dict[str, str],
        python_url: str,
        rust_url: str,
        groups: list[str] | None = None,
    ) -> dict[str, bool]:
        """Run each selected group's Rust contract target once.

        Every target either runs its `cargo test --test TARGET` (which drives
        the external comparator) or, for a group whose Rust target is not yet
        written, the comparator directly. A green cargo run that executed zero
        tests is refused, matching the shared scripts. `groups` overrides the
        default selection so the combined gate can run exactly the one group
        its fresh environment was started for.
        """
        results: dict[str, bool] = {}
        for group in groups if groups is not None else self.target_groups():
            target = GROUPS[group]
            retry_log = ROOT / "work" / "rust-phase1" / f"oracle-retries-{secrets.token_hex(8)}.jsonl"
            retry_log.parent.mkdir(parents=True, exist_ok=True)
            full_env = {
                **env,
                "PYTHON_CONTRACT_URL": python_url,
                "RUST_CONTRACT_URL": rust_url,
                "RUST_CONTRACT_SERVER_BINARY": str(self.server_path()),
                "ORACLE_CAPTURE_URL": python_url,
                "ORACLE_CAPTURE_GROUP": group,
                "ORACLE_RETRY_LOG": str(retry_log),
            }
            if not (ROOT / "rs/contract/tests" / f"{target}.rs").exists():
                command = [
                    self.python_bin,
                    "-m",
                    "scripts.rust_contract.compare",
                    "--group",
                    group,
                ]
            else:
                command = [
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
                ]
            completed = subprocess.run(
                command, cwd=ROOT, env=full_env, check=False, capture_output=True, text=True
            )
            if retry_log.exists():
                self.oracle_retries.extend(json.loads(line) for line in retry_log.read_text().splitlines())
                retry_log.unlink()
            if "ORACLE_UNAVAILABLE" in completed.stdout + completed.stderr:
                self.oracle_unavailable_groups.append(group)
            if completed.stdout:
                print(completed.stdout, end="", flush=True)
            if completed.stderr:
                print(completed.stderr, end="", file=sys.stderr, flush=True)
            passed = completed.returncode == 0
            if passed and command[0] == "cargo" and not re.search(
                r"test result: ok\. [1-9][0-9]* passed", completed.stdout
            ):
                print(
                    f"contract target {group} ran no Rust test: refusing a green result",
                    file=sys.stderr,
                )
                passed = False
            results[group] = passed
            print(f"contract target {group}: {'OK' if passed else 'FAILED'}", flush=True)
        return results

    def finalize(self, results: dict[str, bool]) -> None:
        """Check the assertion ledger and write the run report, then fail.

        The report is written even when the ledger or a target fails, so a
        publication job always has the machine-readable outcome to attach.
        """
        passed = [group for group, ok in results.items() if ok]
        failed = [group for group, ok in results.items() if not ok]
        ledger = None
        ledger_error = None
        if self.require_complete_ledger:
            try:
                ledger = self.check_ledger(passed)
            except RunnerError as error:
                ledger_error = str(error)
        if self.report is not None:
            self.write_report(results, passed, failed, ledger, ledger_error)
        if ledger_error:
            raise RunnerError(ledger_error)
        if failed:
            raise RunnerError(f"contract targets failed: {', '.join(failed)}")

    def check_ledger(self, passed_groups: list[str]) -> dict[str, int]:
        """Prove the assertion ledger is mapped and has no unpassed rows.

        The checker owns the rule: every discovered assertion has a ledger row
        with a target or a reviewed exclusion, and every `complete` row's target
        group appears in the passing set. A pending row is allowed before its
        route task runs.
        """
        evidence = ROOT / "work" / "rust-phase1"
        evidence.mkdir(parents=True, exist_ok=True)
        passed_path = evidence / "ci-passed-groups.json"
        passed_path.write_text(json.dumps(sorted(passed_groups)) + "\n")
        completed = subprocess.run(
            [
                self.python_bin,
                "-m",
                "scripts.rust_contract.check_assertions",
                "--passed-groups",
                str(passed_path),
            ],
            cwd=ROOT,
            env={**os.environ},
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.stdout:
            print(completed.stdout, end="", flush=True)
        if completed.stderr:
            print(completed.stderr, end="", file=sys.stderr, flush=True)
        ledger = json.loads(
            (ROOT / "tests/rust_contract/assertions.json").read_text()
        )
        counts = {"rows": len(ledger["rows"])}
        for row in ledger["rows"]:
            counts[row.get("status", "unknown")] = counts.get(row.get("status", "unknown"), 0) + 1
        if completed.returncode != 0:
            raise RunnerError("assertion ledger is incomplete or unpassed")
        return counts

    def write_report(
        self,
        results: dict[str, bool],
        passed: list[str],
        failed: list[str],
        ledger: dict[str, int] | None,
        ledger_error: str | None = None,
    ) -> None:
        """Write the machine-readable contract-gate report and echo it."""
        report = {
            "source_sha": self.sha,
            "oracle_retries": self.oracle_retries,
            "oracle_unavailable_groups": sorted(set(self.oracle_unavailable_groups)),
            "outcome": "ORACLE_UNAVAILABLE" if self.oracle_unavailable_groups else ("failed" if failed or ledger_error else "passed"),
            "profile": os.environ.get("RUST_CONTRACT_PROFILE", "byte"),
            "rules_sha256": os.environ.get("MEANING_RULES_SHA256"),
            "kyc_execution_gate_enabled": self.gate,
            "group": self.group,
            "rust_target": self.rust_target,
            "mixed_caddy": self.mixed_caddy,
            "require_complete_ledger": self.require_complete_ledger,
            "groups": {
                group: ("passed" if ok else "failed")
                for group, ok in sorted(results.items())
            },
            "passed_groups": sorted(passed),
            "failed_groups": sorted(failed),
            "assertion_ledger": ledger,
            "assertion_ledger_error": ledger_error,
        }
        text = json.dumps(report, indent=2) + "\n"
        path = self.report if self.report.is_absolute() else ROOT / self.report
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        print(f"report written to {path}", flush=True)
        print(text, end="", flush=True)

    def seed_integration_identities(self, env: dict[str, str]) -> None:
        """Provision the accounts the unchanged integration suite logs in as.

        The corpus seed builds market fixtures but not the `itest-*` users, so
        without this every authenticated integration test fails with 401. The
        fixtures are seeded with REAL provenance, the trusted route an
        administrator-invited organization takes, so market writes pass the
        provenance guard. This changes no application code.
        """
        subprocess.run(
            [self.python_bin, "-m", "scripts.rust_contract.seed_integration"],
            cwd=ROOT,
            env={**env, "DISPOSABLE_ITEST_PASSWORD": ITEST_PASSWORD},
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
            "CRUD_SNIPPET": str(ROOT / "deploy/caddy/rust-crud.caddy"),
            "CADDY_PORT": str(port),
            "PYTHON_UPSTREAM": f"127.0.0.1:{python_port}",
            # The Phase 1 harness Rust does not serve the Phase 2 groups, so
            # every CRUD group keeps Python. The pre-auth authority is still the
            # Rust service, because the mixed run proves the shared counter.
            **{f"{group}_UPSTREAM": f"127.0.0.1:{python_port}"
               for group in ("NOTIFICATIONS", "PREFERENCES", "FEEDBACK",
                             "WATCHLISTS", "ALERTS", "REFERRALS")},
            **{f"{group}_UPSTREAM": f"127.0.0.1:{rust_port}"
               for group in ("SIMPLE", "ORDERBOOK", "PRICES", "CURVES", "MCP")},
            "PREAUTH_UPSTREAM": f"127.0.0.1:{rust_port}",
        }
        # One shared config owns forwarding options for both route groups and
        # Python fallback; do not synthesize a plain-proxy variant here.
        config = str(ROOT / "tests/rust_contract/Caddyfile")
        subprocess.run([caddy, "adapt", "--config", config, "--validate"],
                       cwd=ROOT, env=caddy_env, check=True)
        self.spawn([caddy, "run", "--config", config], caddy_env)
        wait_for_port(port)
        url = f"http://127.0.0.1:{port}"
        attest(url, self.token)
        self.seed_integration_identities(env)
        subprocess.run(
            [self.python_bin, "-m", "pytest", "tests/integration", "-q",
             "--run-disposable-integration", "--disposable-target-url", url,
             f"--disposable-target-token={self.token}"],
            cwd=ROOT,
            env={**env, "DISPOSABLE_ITEST_PASSWORD": ITEST_PASSWORD},
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
            if group == "mcp" and os.environ.get("RUST_CONTRACT_PROFILE") != "phase1-meaning-v1":
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
            if os.environ.get("RUST_CONTRACT_PROFILE") == "phase1-meaning-v1":
                from scripts.rust_contract.meaning_runner import (
                    canonicalise_tape_recordings,
                )

                for entries in (first, second):
                    canonicalise_tape_recordings(entries)
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
            if os.environ.get("RUST_CONTRACT_PROFILE") == "phase1-meaning-v1":
                from scripts.rust_contract.record import stable
                document = {"profile": "phase1-meaning-v1", "rules_sha256": os.environ["MEANING_RULES_SHA256"],
                            "source_sha": self.sha, "clock": CORPUS_CLOCK, "group": group,
                            "kyc_gate": self.gate, "entries": stable(entries)}
                destination = ROOT / "tests/rust_contract/phase1_meaning/corpus" / ("enabled" if self.gate else "disabled")
                destination.mkdir(parents=True, exist_ok=True)
                (destination / (group + ".json")).write_text(json.dumps(document, indent=2, default=str) + "\n")
                continue
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

    def run_phase2(self, args) -> None:
        """Use this lifecycle owner for isolated write cells; never a live target."""
        if getattr(args, "phase2_journeys", False):
            from scripts.rust_contract.phase2_journeys import run as run_journeys

            run_journeys(self, args)
            return
        from scripts.rust_contract.phase2_seed import (
            APP_ROLE as role,
        )
        from scripts.rust_contract.phase2_seed import (
            BACKUP_ROLE,
            DATABASES,
            MIGRATOR_ROLE,
            PASSWORD,
            connect,
            digest,
            identifier,
            sequence_marks,
            snapshot,
        )
        from scripts.rust_contract.phase2_state import compare_state, validate_cells

        scenarios = json.loads((ROOT / "tests/rust_contract/phase2/scenarios.json").read_text())["scenarios"]
        from scripts.rust_contract.phase2_scenarios import (
            REGISTRY,
            ScenarioError,
            select_scenarios,
        )

        if args.phase2_measure or args.phase2_order_write_benchmark:
            raise RunnerError("Phase 2 measurement and benchmark routes are not implemented")
        selected = args.phase2_scenario or []
        if not selected or any(name in REGISTRY for name in selected):
            from scripts.rust_contract.phase2_routes import run_registered

            try:
                registered = select_scenarios(args.phase2_group, selected or None)
                run_registered(self, args, registered)
            except ScenarioError as error:
                raise RunnerError(str(error)) from error
            return
        known = {row["id"]: row for row in scenarios}
        if any(name not in known for name in selected):
            raise RunnerError("unknown Phase 2 scenario")
        if args.phase2_measure or args.phase2_order_write_benchmark or args.phase2_races:
            raise RunnerError("Phase 2 measurement, benchmark and race routes are not implemented")
        if args.phase2_group != "all" and any(known[name]["group"] not in {args.phase2_group, "all"} for name in selected):
            raise RunnerError("scenario does not belong to selected group")
        if selected == ["http-boundaries"]:
            environment = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": self.sha,
                           "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars", "DATABASE_URL": "sqlite+aiosqlite:///:memory:"}
            subprocess.run([self.python_bin,"-m","pytest","tests/rust_contract/test_phase2_http.py","-q"],cwd=ROOT,env=environment,check=True)
            subprocess.run(["cargo","test","--manifest-path","rs/Cargo.toml","--locked","-p","verdaxis-read","--test","phase2_http","--test","http_contract"],cwd=ROOT,
                env={**environment,"SQLX_OFFLINE":"true"},check=True)
            if args.report:
                args.report.parent.mkdir(parents=True,exist_ok=True)
                args.report.write_text(json.dumps({"source_sha":self.sha,"scenario":"http-boundaries","scope":"component-contract",
                    "python_exit":0,"rust_exit":0,"cross_process_ingress_qualified":False},indent=2)+"\n")
            return
        if selected == ["auth-token-matrix"]:
            subprocess.run([self.python_bin, "-m", "scripts.rust_contract.phase2_auth_vectors", "--check"],
                cwd=ROOT, env={**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": self.sha,
                              "DATABASE_URL": "sqlite+aiosqlite:///:memory:"}, check=True)
            subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--test", "auth_tokens"],
                cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha, "SQLX_OFFLINE": "true"}, check=True)
            return
        # Phase 2 Task 17: referral reads and public resolution (start)
        if selected == ["referral-reads"]:
            # One migrated disposable database. The Rust test seeds and
            # truncates its own deterministic rows through the admin URL, then
            # drives the real router as the application role.
            port = self.start_database()
            env = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": self.sha,
                   "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars", **self.urls(port)}
            self.seed = False
            self.migrate_and_seed(env)
            app_url = f"postgresql://{APP_ROLE}:{APP_PASSWORD}@127.0.0.1:{port}/{DB_NAME}"
            admin_url = f"postgresql://postgres:disposable_admin_pw@127.0.0.1:{port}/{DB_NAME}"
            subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
                            "--test", "crud_referral_reads"], cwd=ROOT,
                env={**env, "SQLX_OFFLINE": "true", "REQUIRE_REFERRAL_INTEROP": "true",
                     "REFERRAL_INTEROP_APP_URL": app_url, "REFERRAL_INTEROP_ADMIN_URL": admin_url,
                     "REFERRAL_INTEROP_TIME_ZONE": ORACLE_TIMEZONE}, check=True)
            if args.report:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(json.dumps({"source_sha": self.sha, "scenario": "referral-reads",
                    "scope": "component-contract", "rust_exit": 0, "route_ids": ["R02", "R03", "R05"],
                    "databases": [DB_NAME], "cross_process_ingress_qualified": False,
                    "note": "single disposable database; the twin-stack response and state differential belongs to the Phase 2 transport owner"}, indent=2) + "\n")
            return
        # Phase 2 Task 17: referral reads and public resolution (end)
        if selected not in (["harness-smoke"], ["auth-state-matrix"], ["watchlist-reads"]):
            raise RunnerError("select one implemented Phase 2 scenario")
        port = self.start_database()
        policy_dir = "/tmp/phase2-role-policy"
        docker("exec", self.container, "mkdir", "-p", policy_dir)
        for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql", "bootstrap_roles.sql", "validate_roles.sql"):
            docker("cp", str(ROOT / "deploy/postgres" / name), f"{self.container}:{policy_dir}/{name}")
        # Create the roles and move schema ownership to the migrator before the
        # migration, exactly as production does. Metadata functions declared
        # SECURITY DEFINER then run with migrator authority; migrating as the
        # app role left them unable to bump the cache generation.
        docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", DB_NAME,
            "-v", f"database_name={DB_NAME}", "-v", f"app_role={role}", "-v", f"migrator_role={MIGRATOR_ROLE}",
            "-v", f"backup_role={BACKUP_ROLE}", "-f", f"{policy_dir}/bootstrap_roles.sql")
        docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", DB_NAME,
            "-c", f"ALTER ROLE {identifier(MIGRATOR_ROLE)} PASSWORD '{PASSWORD}'")
        migrator_url = f"postgresql+asyncpg://{MIGRATOR_ROLE}:{PASSWORD}@127.0.0.1:{port}/{DB_NAME}"
        env = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": self.sha,
               "JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars", **self.urls(port),
               "DATABASE_URL": migrator_url, "MIGRATOR_DATABASE_URL": migrator_url}
        self.seed = False
        self.migrate_and_seed(env)
        # Seed one immutable source schema before cloning. Neither request cell
        # owns its schema. Apply the same audited role policy in each clone.
        with connect(port, DB_NAME) as source:
            source.execute("INSERT INTO organizations (id,name,domain,type) VALUES ('10000000-0000-4000-8000-000000000001','Phase 2 fixture','phase2.invalid','FUEL_BUYER')")
            source.execute("INSERT INTO users (id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES ('20000000-0000-4000-8000-000000000001','actor@phase2.invalid','not-a-login','BUYER','APPROVED','10000000-0000-4000-8000-000000000001',true,false)")
        with connect(port, "postgres", autocommit=True) as admin:
            for database in DATABASES:
                admin.execute(f"CREATE DATABASE {identifier(database)} TEMPLATE {identifier(DB_NAME)}")
        for database in DATABASES:
            for script in ("bootstrap_roles.sql", "validate_roles.sql"):
                docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", database,
                    "-v", f"database_name={database}", "-v", f"app_role={role}", "-v", f"migrator_role={MIGRATOR_ROLE}",
                    "-v", f"backup_role={BACKUP_ROLE}", "-f", f"{policy_dir}/{script}")
        with connect(port, DATABASES[0]) as admin:
            admin.execute(f"ALTER ROLE {identifier(role)} PASSWORD '{PASSWORD}'")
        if selected == ["watchlist-reads"]:
            subprocess.run(["cargo","test","--manifest-path","rs/Cargo.toml","--locked","-p","verdaxis-read","--test","crud_watchlist_reads"],cwd=ROOT,
                env={**env,"RELEASE_SHA":self.sha,"SQLX_OFFLINE":"true",
                    "AUTH_INTEROP_ADMIN_URL":f"postgresql://postgres:disposable_admin_pw@127.0.0.1:{port}/{DATABASES[0]}"},check=True)
            return
        if selected == ["auth-state-matrix"]:
            subprocess.run(["cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--test", "auth_transactions", "--test", "auth_transaction_panic", "--test", "auth_tokens", "--", "--nocapture"], cwd=ROOT,
                env={**env, "RELEASE_SHA": self.sha, "SQLX_OFFLINE": "true", "REQUIRE_AUTH_INTEROP": "true",
                     "AUTH_INTEROP_APP_URL": f"postgresql://{role}:{PASSWORD}@127.0.0.1:{port}/{DATABASES[0]}",
                     "AUTH_INTEROP_ADMIN_URL": f"postgresql://postgres:disposable_admin_pw@127.0.0.1:{port}/{DATABASES[0]}"}, check=True)
            return
        initial = []
        for database in DATABASES:
            with connect(port, database) as reader:
                initial.append(snapshot(reader))
        if compare_state(*initial):
            raise RunnerError("twin initial logical state differs")
        acl = digest({name: (ROOT / "deploy/postgres" / name).read_text() for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql")})
        validate_cells({"database": DATABASES[0], "seed": digest(initial[0]), "acl": acl},
                       {"database": DATABASES[1], "seed": digest(initial[1]), "acl": acl}, self.token, f"http://127.0.0.1:{ephemeral_port()}")
        import urllib.error
        import urllib.request
        from datetime import UTC, datetime

        from scripts.rust_contract.compare import compare_responses
        subprocess.run(["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--features", "contract-tests", "--example", "phase2_transport"],
            cwd=ROOT, env={**os.environ, "RELEASE_SHA": self.sha, "SQLX_OFFLINE": "true"}, check=True)
        endpoints = []
        for runtime, database in zip(("python", "rust"), DATABASES):
            bind_port = ephemeral_port()
            probe_env = {**env, "DISPOSABLE_TEST_TOKEN": self.token, "PHASE2_DATABASE_URL": f"postgresql://{role}:{PASSWORD}@127.0.0.1:{port}/{database}",
                         "PHASE2_BIND": f"127.0.0.1:{bind_port}"}
            command = [self.python_bin, "-m", "tests.rust_contract.server", "--port", str(bind_port)] if runtime == "python" else [str(ROOT / "rs/target/debug/examples/phase2_transport")]
            self.spawn(command, probe_env)
            wait_for_port(bind_port)
            endpoint = f"http://127.0.0.1:{bind_port}"
            attest(endpoint, self.token)
            endpoints.append(endpoint)
        actions = []
        for action, fault in (("insert-feedback", "none"), ("between-writes", "between-writes"), ("after-commit", "after-commit")):
            request_action = {"ids": [f"30000000-0000-4000-8000-{len(actions)+1:012d}", f"40000000-0000-4000-8000-{len(actions)+1:012d}"],
                              "now": f"2026-10-18T00:00:00.{len(actions)+1:06d}+00:00", "referral_code": "VDX-ABC123", "fault": fault}
            responses, clock_evidence = [], []
            for endpoint in endpoints:
                before = datetime.now(UTC)
                request = urllib.request.Request(endpoint + "/__contract__/phase2/write", data=json.dumps(request_action).encode(),
                    headers={"content-type": "application/json", "x-disposable-token": self.token}, method="POST")
                try:
                    response = urllib.request.urlopen(request, timeout=20)
                except urllib.error.HTTPError as error:
                    response = error
                with response:
                    raw = response.read()
                    database_now = datetime.fromisoformat(response.headers["x-database-now"])
                    if not before <= database_now <= datetime.now(UTC):
                        raise RunnerError("database now outside captured request interval")
                    responses.append({"status": response.status, "headers": {"content-type": response.headers["content-type"]}, "body": raw})
                    clock_evidence.append({"database_now": database_now.isoformat(), "action_clock": request_action["now"]})
            if compare_responses(*responses, profile="phase2-meaning-v1", route_id="N02", rules_sha256=__import__("hashlib").sha256((ROOT / "tests/rust_contract/phase2/normalization.json").read_bytes()).hexdigest()):
                raise RunnerError("write transport responses differ")
            states = []
            for database in DATABASES:
                with connect(port, database) as reader:
                    states.append(snapshot(reader))
            differences = compare_state(*states)
            if differences:
                raise RunnerError("committed twin state differs")
            expected_rows = 2 if len(actions) < 2 else 4
            if len(states[0]["feedback_entries"]["rows"]) != expected_rows:
                raise RunnerError("fault schedule left partial or replayed child rows")
            actions.append({"id": action, "outcome": "rolled-back" if fault == "between-writes" else "ambiguous" if fault == "after-commit" else "committed",
                            "database_clock": clock_evidence,
                            "python_state_sha256": digest(states[0]), "rust_state_sha256": digest(states[1]), "differences": differences})
        sequence_evidence = {}
        for database in DATABASES:
            with connect(port, database) as reader:
                sequence_evidence[database] = sequence_marks(reader)
        mixed_database = "phase2_mixed_test"
        # Probe ASGI lifespan can hold an application pool on the seed. Stop
        # only this runner's servers before cloning the untouched seed again.
        for child in self.children:
            child.terminate()
        for child in self.children:
            child.wait(timeout=15)
        self.children = []
        with connect(port, "postgres", autocommit=True) as admin:
            admin.execute(f"CREATE DATABASE {identifier(mixed_database)} TEMPLATE {identifier(DB_NAME)}")
        for script in ("bootstrap_roles.sql", "validate_roles.sql"):
            docker("exec", self.container, "psql", "-X", "-U", "postgres", "-d", mixed_database,
                "-v", f"database_name={mixed_database}", "-v", f"app_role={role}", "-v", f"migrator_role={MIGRATOR_ROLE}",
                "-v", f"backup_role={BACKUP_ROLE}", "-f", f"{policy_dir}/{script}")
        mixed_url = f"postgresql://{role}:{PASSWORD}@127.0.0.1:{port}/{mixed_database}"
        mixed_endpoints = []
        for runtime in ("python", "rust"):
            bind_port = ephemeral_port()
            probe_env = {**env, "DISPOSABLE_TEST_TOKEN": self.token, "PHASE2_DATABASE_URL": mixed_url,
                         "PHASE2_BIND": f"127.0.0.1:{bind_port}", "PHASE2_EMIT_EVENTS": "true"}
            command = [self.python_bin, "-m", "tests.rust_contract.server", "--port", str(bind_port)] if runtime == "python" else [str(ROOT / "rs/target/debug/examples/phase2_transport")]
            self.spawn(command, probe_env)
            wait_for_port(bind_port)
            endpoint = f"http://127.0.0.1:{bind_port}"
            attest(endpoint, self.token)
            mixed_endpoints.append(endpoint)
        for index, endpoint in enumerate(mixed_endpoints):
            action = {"ids": [f"50000000-0000-4000-8000-{index+1:012d}", f"60000000-0000-4000-8000-{index+1:012d}"],
                      "now": "2026-10-18T00:00:01+00:00", "referral_code": "VDX-ABC123", "fault": "none"}
            request = urllib.request.Request(endpoint + "/__contract__/phase2/write", data=json.dumps(action).encode(),
                headers={"content-type": "application/json", "x-disposable-token": self.token}, method="POST")
            with urllib.request.urlopen(request, timeout=20) as response:
                if response.status != 200:
                    raise RunnerError("mixed writer refused")
                response.read()
        dispatched = subprocess.run([self.python_bin, "-m", "tests.rust_contract.phase2.mixed_dispatch"], cwd=ROOT,
            env={**env, "PHASE2_DATABASE_URL": mixed_url}, check=True, capture_output=True, text=True)
        mixed_evidence = json.loads(dispatched.stdout.strip().splitlines()[-1])
        with connect(port, mixed_database) as reader:
            mixed_evidence["sequence_high_water"] = sequence_marks(reader)
            if len(snapshot(reader)["feedback_entries"]["rows"]) != 4:
                raise RunnerError("mixed writers lost committed rows")
        if args.report:
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps({"source_sha": self.sha, "scenario": "harness-smoke", "scope": "harness-only",
                "sequence_high_water": sequence_evidence, "mixed_writer_dispatch": mixed_evidence,
                "route_qualification": False, "databases": list(DATABASES), "seed_sha256": digest(initial[0]), "acl_sha256": acl,
                "actions": actions, "dispatchers": "not-started"}, indent=2) + "\n")

    def stop_children(self) -> None:
        for child in self.children:
            child.terminate()
        for child in self.children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        self.children = []

    def cleanup(self) -> None:
        self.stop_children()
        if self.container and not self.keep:
            # -v removes the anonymous data volume; a forced remove skips the --rm cleanup.
            owned = self.container
            docker("rm", "-f", "-v", owned)
            if docker("ps", "-aq", "--filter", f"id={owned}").strip():
                raise RunnerError("owned disposable container remains after cleanup")
        self.container = ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--group", default="health", choices=sorted([*GROUPS, "all"]))
    parser.add_argument(
        "--rust-target",
        choices=sorted([*GROUPS, "all"]),
        default=None,
        help="Rust contract target(s) to run; `all` runs every target",
    )
    parser.add_argument("--mixed-caddy", action="store_true")
    parser.add_argument("--record-all", action="store_true")
    parser.add_argument(
        "--require-complete-ledger",
        action="store_true",
        help="fail unless every discovered assertion is mapped and no complete row is unpassed",
    )
    parser.add_argument(
        "--report",
        type=Path,
        default=None,
        help="write a machine-readable contract-gate report to this path",
    )
    parser.add_argument(
        "--kyc-gate", choices=("enabled", "disabled"), default="enabled"
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="build and run the Rust contract server with cargo's release profile "
        "(for latency measurement; correctness runs stay on the dev profile)",
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
    parser.add_argument("--phase2", action="store_true")
    # Phase 2 Task 20: register the order-write boundary group start
    parser.add_argument("--phase2-group", choices=("all", "notifications", "preferences", "feedback", "watchlists", "alerts", "referrals", "order-write-boundary"), default="all")
    # Phase 2 Task 20: register the order-write boundary group end
    parser.add_argument("--phase2-scenario", action="append")
    parser.add_argument("--phase2-size", choices=("small", "large"), default="small")
    parser.add_argument("--phase2-races", action="store_true")
    parser.add_argument("--phase2-journeys", action="store_true",
                        help="run the six frontend client journeys against the Rust candidate")
    parser.add_argument("--phase2-measure", action="store_true")
    parser.add_argument("--phase2-order-write-benchmark", action="store_true")
    parser.add_argument("--release-binary", type=Path)
    parser.add_argument("--server-binary", type=Path, help="Built contract_server path, including a shared Cargo target directory")
    parser.add_argument("--profile", choices=("phase1-meaning-v1",))
    parser.add_argument("--rules-sha256")
    args = parser.parse_args(argv)
    if args.profile:
        from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256
        if args.rules_sha256 != PHASE1_RULES_SHA256:
            parser.error("reviewed successor rules digest is required")
        os.environ["RUST_CONTRACT_PROFILE"] = args.profile
        os.environ["MEANING_RULES_SHA256"] = args.rules_sha256
    elif args.rules_sha256:
        parser.error("rules digest requires an explicit profile")
    if args.phase2_journeys and not args.phase2:
        parser.error("--phase2-journeys requires --phase2")
    if args.phase2_journeys and (args.phase2_races or args.phase2_measure or args.phase2_order_write_benchmark):
        parser.error("--phase2-journeys runs alone")
    if args.phase2:
        scenarios = json.loads((ROOT / "tests/rust_contract/phase2/scenarios.json").read_text())["scenarios"]
        from scripts.rust_contract.phase2_scenarios import REGISTRY
        known = {row["id"] for row in scenarios} | REGISTRY.keys()
        if any(name not in known for name in (args.phase2_scenario or [])):
            parser.error("unknown Phase 2 scenario")
    if args.mixed_caddy and args.group != "all":
        parser.error("--mixed-caddy requires --group all")
    if args.mixed_caddy and args.record_all:
        parser.error("--mixed-caddy cannot record golden data")
    if args.group == "all" and not (args.mixed_caddy or args.rust_target or args.record_all):
        parser.error("--group all requires --mixed-caddy, --rust-target or --record-all")
    if args.rust_target and args.record_all:
        parser.error("--rust-target cannot record golden data")
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
        rust_target=args.rust_target,
        require_complete_ledger=args.require_complete_ledger,
        report=args.report,
        release=args.release,
        server_binary=args.server_binary,
    )
    try:
        if args.phase2:
            runner.run_phase2(args)
        else:
            runner.run()
    except (RunnerError, subprocess.CalledProcessError) as error:
        print(f"disposable run failed: {error}", file=sys.stderr)
        return 1
    finally:
        runner.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
