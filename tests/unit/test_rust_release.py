"""Task 12b: the dual-service staging deploy transaction.

Every test runs scripts/deploy.sh as a real subprocess against a disposable
checkout with fake sudo/systemctl/curl and fixture artifact bytes. Nothing here
touches a real host, a real systemd unit, or a real database; the Rust fixture
binary is a shell script that only records its execution into the ledger.
"""
from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
UNIT_MANIFEST = "deploy/systemd/runtime-units.manifest"
STAGING_POLICY = "deploy/rust/staging-policy.json"
PROVENANCE_KEY = "deploy/rust/artifact-provenance.pub.pem"
RUST_UNIT = "deploy/systemd/verdaxis-rust-staging.service"
DEPLOY_START_GUARD = "scripts/verify_deploy_start_guard.py"

SOURCE_SHA = "a" * 40
MIGRATION_EXPECTED = "kycg_20261014_staging"
MIGRATION_TARGET = "kycd_20261015_staging"

# The command sequence a successful Python-only deploy must keep, recorded
# from the transaction before Rust support existed (Task 12a tree, 9c249ea5),
# in its true execution order. A production or staging deploy without a Rust
# artifact must produce exactly this ledger and nothing Rust-side; this is the
# PM's no-change proof for production. `sudo systemctl stop` is failure-path
# only and is asserted by the failure-injection tests instead.
GOLDEN_PYTHON_LEDGER = (
    "python:scripts/verify_migration_revision.py",
    "python:scripts/preflight_runtime.py",
    "python:-m pip install -r requirements.txt -c constraints.txt",
    "python:-m pip check",
    "python:scripts/apply_migration_checkpoint.py",
    "/scripts/converge_runtime_acls.py --environment staging",
    "sudo:systemctl restart",
    "python:scripts/validate_health_response.py",
)


def ledger_contains(ledger: list[str], entry: str) -> bool:
    """Ledger entries are full command lines; match by prefix/substring."""
    return any(entry in line for line in ledger)

def _load_verifier():
    spec = importlib.util.spec_from_file_location(
        "verify_rust_artifact", ROOT / "scripts" / "verify_rust_artifact.py"
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


_PREPARER = None


def _load_preparer():
    """The one loaded instance: two loads would define two distinct error classes."""
    global _PREPARER
    if _PREPARER is None:
        spec = importlib.util.spec_from_file_location(
            "prepare_rust_runtime", ROOT / "scripts" / "prepare_rust_runtime.py"
        )
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        _PREPARER = module
    return _PREPARER


def _sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _git(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], cwd=cwd, check=True, capture_output=True, text=True
    )


def _write_executable(path: Path, source: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source)
    path.chmod(0o755)


class RustReleaseFixture:
    """A disposable deployment checkout with dual-service attestation.

    The provenance keypair is a throwaway Ed25519 key generated in the
    temporary directory; the repository's pinned public key is copied in with
    its digest so the transaction's pin check exercises the real bytes.
    """

    def __init__(self, tmp_path: Path):
        from cryptography.hazmat.primitives.asymmetric.ed25519 import (
            Ed25519PrivateKey,
        )
        from cryptography.hazmat.primitives.serialization import (
            Encoding,
            NoEncryption,
            PrivateFormat,
            PublicFormat,
        )

        self.tmp_path = tmp_path
        self.verifier = _load_verifier()
        self.origin = tmp_path / "origin.git"
        self.source = tmp_path / "approved-source"
        # The checkout deliberately sits OUTSIDE the canonical `<env>/be`
        # layouts: deploy.sh resets PATH for those paths, which would replace
        # the fixture's fake `sudo`, `curl` and `systemctl` with the host's
        # real commands. A non-canonical checkout is deployable through the
        # documented override (TARGET_BRANCH, SERVICE_NAME, HEALTH_URL,
        # DEPLOY_ENVIRONMENT) that `deploy_environment` supplies.
        self.checkout = tmp_path / "deploy-checkout"
        for directory in (self.origin, self.source):
            directory.mkdir()
        _git(self.origin, "init", "--bare", "-q")
        _git(self.source, "init", "-q", "-b", "staging")
        _git(self.source, "config", "user.email", "release-test@example.invalid")
        _git(self.source, "config", "user.name", "Release Test")

        self.key = Ed25519PrivateKey.generate()
        self.key.public_key().public_bytes(
            Encoding.PEM, PublicFormat.SubjectPublicKeyInfo
        )
        self.public_key = self.key.public_key().public_bytes(
            Encoding.PEM, PublicFormat.SubjectPublicKeyInfo
        )
        self.private_key = self.key.private_bytes(
            Encoding.PEM, PrivateFormat.PKCS8, NoEncryption()
        )

        self._write_source_tree()
        _git(self.source, "add", ".")
        _git(self.source, "commit", "-qm", "old release")
        _git(self.source, "remote", "add", "origin", str(self.origin))
        _git(self.source, "push", "-q", "-u", "origin", "staging")
        _git(
            tmp_path,
            "clone",
            "-q",
            "--branch",
            "staging",
            str(self.origin),
            str(self.checkout),
        )
        self.old_sha = _git(self.checkout, "rev-parse", "HEAD").stdout.strip()
        self._write_fake_tools()

    # -- source tree -------------------------------------------------------
    def _write_source_tree(self) -> None:
        source = self.source
        scripts = source / "scripts"
        scripts.mkdir()
        shutil.copy2(ROOT / "scripts/deploy.sh", scripts / "deploy.sh")
        for name in (
            "verify_deploy_start_guard.py",
            "converge_runtime_acls.py",
            "prepare_rust_budget.py",
            "prepare_rust_runtime.py",
            "verify_rust_artifact.py",
            "validate_health_response.py",
        ):
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        (scripts / "preflight_runtime.py").write_text("# release test preflight\n")
        for name in ("verify_migration_revision.py", "apply_migration_checkpoint.py"):
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        # The real budget helper imports the deployed application settings in a
        # fresh subprocess, so the fixture tree carries the config module.
        app_dir = source / "app"
        app_dir.mkdir()
        (app_dir / "__init__.py").write_text("")
        shutil.copy2(ROOT / "app/config.py", app_dir / "config.py")

        systemd = source / "deploy/systemd"
        systemd.mkdir(parents=True)
        (systemd / "runtime-units.manifest").write_text(
            "staging\tverdaxis-backend-staging.service\n"
            "staging\tverdaxis-rust-staging.service\n"
        )
        unit_body = "\n[Service]\nType=simple\nExecStart=/bin/true\n"
        (systemd / "verdaxis-backend-staging.service").write_text(
            "[Unit]\nDescription=python backend fixture\n" + unit_body
        )
        # Keep the shipped launch and sandbox contract in release archives.
        # The real verifier's missing-artifact case lives in test_rust_units.
        shutil.copy2(ROOT / RUST_UNIT, source / RUST_UNIT)

        rust = source / "deploy/rust"
        rust.mkdir(parents=True)
        (rust / "staging-policy.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "environment": "staging",
                    "rust_enabled": False,
                    "service_name": "verdaxis-rust-staging.service",
                    "bind": "127.0.0.1:8011",
                    "health_url": "http://127.0.0.1:8011/health/ready",
                    "db_session_time_zone": "UTC",
                    "staged_releases_dir": ".runtime-rust/releases",
                },
                indent=2,
                sort_keys=True,
            )
            + "\n"
        )
        (rust / "artifact-provenance.pub.pem").write_bytes(self.public_key)

        (source / "deploy/migration-checkpoints.tsv").write_text(
            f"{MIGRATION_EXPECTED}\t{MIGRATION_TARGET}\n{MIGRATION_TARGET}\t{MIGRATION_TARGET}\n"
        )
        postgres_policy = source / "deploy/postgres"
        postgres_policy.mkdir(parents=True)
        for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql"):
            shutil.copy2(ROOT / "deploy/postgres" / name, postgres_policy / name)
        (source / ".gitignore").write_text(
            "venv/\n.env\n__pycache__/\n.runtime-release.env\n"
            ".runtime-monitor.env\n.runtime-deploy/\n.runtime-rust/\n"
        )
        (source / "requirements.txt").write_text("release-test==1\n")
        (source / "constraints.txt").write_text("release-test==1\n")
        # A deployed .env carrying exactly the identity keys the real Rust
        # preparation and ACL helpers project. It stays out of git.
        (source / ".env").write_text(
            "ENVIRONMENT=staging\n"
            "RELEASE_SHA=placeholder-written-per-deploy\n"
            "JWT_SECRET=" + "k" * 40 + "\n"
            "JWT_ISSUER=verdaxis-staging-api\n"
            "JWT_AUDIENCE=verdaxis-staging-web\n"
            "DATABASE_HOST=127.0.0.1\n"
            "DATABASE_PORT=5432\n"
            "DATABASE_NAME=verdaxis_staging\n"
            "DATABASE_USER=verdaxis_app_staging\n"
            "DATABASE_PASSWORD=fixture-password\n"
            "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging"
            ":fixture-password@127.0.0.1:5432/verdaxis_staging\n"
            "MIGRATOR_DATABASE_URL=postgresql+asyncpg://verdaxis_migrator_staging"
            ":fixture-password@127.0.0.1:5432/verdaxis_staging\n"
            "KYC_EXECUTION_GATE_ENABLED=false\n"
        )

    def _write_fake_tools(self) -> None:
        self.log_path = self.tmp_path / "commands.log"
        fake_bin = self.tmp_path / "fake-bin"
        self.fake_bin = fake_bin

        # The fake Python records every invocation into the ledger. The Rust
        # artifact verifier and runtime preparer are executed for real so the
        # transaction's provenance and projection logic is what runs under
        # test; every other helper stays fake.
        fake_python = """#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "-I" && "${2:-}" == "-c" ]]; then
    # A nested trusted-settings import from a real helper: run the real
    # interpreter for the inline code instead of this ledger wrapper.
    exec __REAL_PYTHON__ "$@"
fi
printf 'python:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "${DEPLOY_FAIL_PHASE:-}" == "preflight" && "${1:-}" == "scripts/preflight_runtime.py" ]]; then exit 41; fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "budget" && "${1:-}" == "scripts/prepare_rust_budget.py" ]]; then exit 47; fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "rust-runtime" && "${1:-}" == *"prepare_rust_runtime.py" ]]; then exit 48; fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "health" && "${1:-}" == "scripts/validate_health_response.py" ]]; then exit 44; fi
case "${1:-}" in
  *verify_rust_artifact.py|*prepare_rust_runtime.py|*prepare_rust_budget.py)
      exec __REAL_PYTHON__ "$@"
      ;;
esac
if [[ "${DEPLOY_BLOCK_PHASE:-}" == "${1:-}" ]]; then sleep 30; fi
"""
        # The fixture's `venv/bin/python` wrapper must work in a clean checkout.
        # GitHub CI installs the dependencies into the interpreter that runs
        # pytest and carries no repository `venv/`, so the real interpreter
        # behind the wrapper is the test process's own (`sys.executable`).
        # A hard-coded `/usr/bin/python3` would fail the real helpers' `dotenv`
        # import, exactly as the draft PR #86 `test` job showed.
        real_python = sys.executable
        fake_python = fake_python.replace("__REAL_PYTHON__", real_python)
        _write_executable(self.checkout / "venv/bin/python", fake_python)
        _write_executable(
            self.checkout / "venv/bin/alembic",
            "#!/usr/bin/env bash\nset -euo pipefail\nprintf 'alembic:%s\\n' \"$*\" >> \"${DEPLOY_TEST_LOG:?}\"\n",
        )
        # Digest pins use the system sha256sum directly; no fake is needed
        # because the digest comparison happens in deploy.sh itself.
        _write_executable(
            fake_bin / "sudo",
            """#!/usr/bin/env bash
set -euo pipefail
printf 'sudo:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "${DEPLOY_FAIL_PHASE:-}" == "systemctl-restart" ]]; then exit 50; fi
""",
        )
        # curl prints a valid readiness body for both services; when
        # DEPLOY_RUST_UNREADY=1 every call after the first fails, so the Rust
        # readiness loop exhausts HEALTH_ATTEMPTS and the deploy fails.
        _write_executable(
            fake_bin / "curl",
            """#!/usr/bin/env bash
set -euo pipefail
printf 'curl:%s\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
counter="${DEPLOY_TEST_LOG:?}.curl-count"
count="$(cat "$counter" 2>/dev/null || printf 0)"
printf '%s' "$(( count + 1 ))" > "$counter"
if [[ "${DEPLOY_RUST_UNREADY:-}" == "1" && "$count" -ge 1 ]]; then
    exit 7
fi
printf '%s' "${DEPLOY_CURL_BODY:-}"
""",
        )
        _write_executable(
            fake_bin / "systemctl",
            """#!/usr/bin/env bash
set -euo pipefail
printf 'systemctl:%s\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "$*" == "show --property=LoadState --value verdaxis-rust-staging.service" ]]; then
    printf '%s\n' "${DEPLOY_RUST_LOAD_STATE:-loaded}"
fi
exit 0
""",
        )

    # -- artifact material -------------------------------------------------
    def build_artifact(self, release_sha: str) -> None:
        """Build and attest a fixture archive bound to `release_sha`."""
        self.artifact_root = self.tmp_path / "rust-artifact"
        data_dir = self.artifact_root / "data"
        data_dir.mkdir(parents=True)
        policy = {"source_sha": "b" * 40, "products": [], "points": []}
        (data_dir / "market-policy.json").write_text(json.dumps(policy) + "\n")
        (data_dir / "market-policy.manifest.json").write_text(
            json.dumps(
                {
                    "source_sha": policy["source_sha"],
                    "policy": "market-policy.json",
                    "policy_sha256": _sha256((data_dir / "market-policy.json").read_bytes()),
                }
            )
            + "\n"
        )
        (data_dir / "allowed-revisions.json").write_text(
            json.dumps([MIGRATION_TARGET]) + "\n"
        )
        binary = self.artifact_root / "verdaxis-read"
        # A fixture binary that would leave a marker if anything executed it.
        binary.write_text(
            "#!/bin/sh\nprintf 'rust-binary:%s\\n' \"$*\" >> \"${DEPLOY_TEST_LOG:?}\"\n"
            "printf '%s\\n' \"{"
            "\\\"binary\\\":\\\"verdaxis-read\\\",\\\"artifact_version\\\":1,"
            "\\\"release_sha\\\":\\\"" + release_sha + "\\\","
            "\\\"migration_revision_allowlist\\\":[\\\"" + MIGRATION_TARGET + "\\\"],"
            "\\\"libc_min\\\":\\\"2.35\\\",\\\"arch\\\":\\\"x86_64\\\",\\\"os\\\":\\\"linux\\\"}\"\n"
        )
        binary.chmod(0o755)
        lockfile = self.artifact_root / "Cargo.lock"
        lockfile.write_text("# fixture lockfile\n")
        archive_bytes = self.verifier.build_archive(
            binary=binary,
            data_dir=data_dir,
            source_sha=release_sha,
            target=self.verifier.DEFAULT_TARGET,
            toolchain="1.95.0",
            lockfile=lockfile,
        )
        archive = self.artifact_root / "release.tar"
        archive.write_bytes(archive_bytes)
        inspected = self.verifier.inspect_archive(
            archive_bytes, release_sha, self.verifier.DEFAULT_TARGET
        )
        record = {
            "version": 1,
            "repository": self.verifier.REPOSITORY,
            "workflow_path": self.verifier.WORKFLOW_PATH,
            "workflow_ref": "refs/heads/staging",
            "source_sha": release_sha,
            "target": self.verifier.DEFAULT_TARGET,
            "archive_sha256": inspected["archive_sha256"],
            "binary_sha256": inspected["binary_sha256"],
            "attested_subjects": ["verdaxis-read", "archive"],
            "gh_attestation_verified": True,
            "attested_at": "2026-10-17T00:00:00Z",
        }
        record_bytes = json.dumps(record, indent=2, sort_keys=True).encode() + b"\n"
        (self.artifact_root / "provenance-record.json").write_bytes(record_bytes)
        (self.artifact_root / "provenance-record.sig").write_bytes(
            self.key.sign(record_bytes)
        )

    # -- releases ----------------------------------------------------------
    def push_rust_release(self, *, enabled: bool) -> str:
        """Publish a candidate release whose committed policy selects Rust."""
        policy = json.loads((self.source / STAGING_POLICY).read_text())
        policy["rust_enabled"] = enabled
        # A release marker keeps every push a real commit even when the policy
        # value equals the previous release's.
        policy["release_note"] = f"rust={'on' if enabled else 'off'}-{time.time_ns()}"
        (self.source / STAGING_POLICY).write_text(
            json.dumps(policy, indent=2, sort_keys=True) + "\n"
        )
        _git(self.source, "add", "-A")
        message = "enable rust" if enabled else "disable rust"
        _git(self.source, "commit", "-qm", message)
        _git(self.source, "push", "-q", "origin", "staging")
        return _git(self.source, "rev-parse", "HEAD").stdout.strip()

    def push_new_release(self) -> str:
        (self.source / "release.txt").write_text(
            f"new release {time.time_ns()}\n"
        )
        _git(self.source, "add", "-A")
        _git(self.source, "commit", "-qm", "new release")
        _git(self.source, "push", "-q", "origin", "staging")
        return _git(self.source, "rev-parse", "HEAD").stdout.strip()

    # -- execution ---------------------------------------------------------
    def deploy_environment(self, release_sha: str, *, fail_phase: str = "") -> dict:
        environment = {
            **os.environ,
            "TARGET_BRANCH": "staging",
            "SERVICE_NAME": "verdaxis-backend-staging-test.service",
            "HEALTH_URL": "https://health.invalid/health/ready",
            "DEPLOY_ENVIRONMENT": "staging",
            "DEPLOY_TEST_LOG": str(self.log_path),
            "DEPLOY_FAIL_PHASE": fail_phase,
            "DEPLOY_STATE_DIR": str(self.tmp_path / "runtime-deploy-state"),
            "HEALTH_ATTEMPTS": "1",
            "APPROVED_RELEASE_SHA": release_sha,
            "MIGRATION_APPROVED_SOURCE_SHA": release_sha,
            "MIGRATION_EXPECTED_CURRENT_REVISION": MIGRATION_EXPECTED,
            "MIGRATION_TARGET_REVISION": MIGRATION_TARGET,
            "DEPLOY_CURL_BODY": json.dumps(
                {
                    "status": "ok",
                    "db": "ok",
                    "environment": "staging",
                    "release_sha": release_sha,
                }
            ),
            "PATH": f"{self.fake_bin}{os.pathsep}{os.environ['PATH']}",
        }
        if getattr(self, "artifact_root", None) is not None:
            environment["RUST_ARTIFACT_ARCHIVE"] = str(
                self.artifact_root / "release.tar"
            )
            environment["RUST_PROVENANCE_RECORD"] = str(
                self.artifact_root / "provenance-record.json"
            )
            environment["RUST_PROVENANCE_SIGNATURE"] = str(
                self.artifact_root / "provenance-record.sig"
            )
        return environment

    def dry_run_environment(self) -> dict:
        environment = self.deploy_environment(self.old_sha)
        return environment

    def deploy(self, release_sha: str, *, fail_phase: str = "", **overrides) -> subprocess.CompletedProcess[str]:
        self._write_deployed_env(release_sha)
        environment = self.deploy_environment(release_sha, fail_phase=fail_phase)
        environment.update(overrides)
        return subprocess.run(
            ["bash", "scripts/deploy.sh"],
            cwd=self.checkout,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )

    def _write_deployed_env(self, release_sha: str) -> None:
        """The deployed .env names the release being deployed, as on the host."""
        (self.checkout / ".env").write_text(self.deployed_env_text(release_sha))

    def deployed_env_text(self, release_sha: str, *, kyc_gate: str | None = "false") -> str:
        """The deployed `.env` body, with the Private Beta KYC switch state."""
        body = (
            "ENVIRONMENT=staging\n"
            f"RELEASE_SHA={release_sha}\n"
            "JWT_SECRET=" + "k" * 40 + "\n"
            "JWT_ISSUER=verdaxis-staging-api\n"
            "JWT_AUDIENCE=verdaxis-staging-web\n"
            "DATABASE_HOST=127.0.0.1\n"
            "DATABASE_PORT=5432\n"
            "DATABASE_NAME=verdaxis_staging\n"
            "DATABASE_USER=verdaxis_app_staging\n"
            "DATABASE_PASSWORD=fixture-password\n"
            "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging"
            ":fixture-password@127.0.0.1:5432/verdaxis_staging\n"
            "MIGRATOR_DATABASE_URL=postgresql+asyncpg://verdaxis_migrator_staging"
            ":fixture-password@127.0.0.1:5432/verdaxis_staging\n"
        )
        return body if kyc_gate is None else body + f"KYC_EXECUTION_GATE_ENABLED={kyc_gate}\n"

    def prepare_runtime_environment(
        self, release_sha: str, *, kyc_gate: str | None = "false"
    ) -> Path:
        """Run the real Rust runtime preparation over the deployed `.env`."""
        environment_file = self.checkout / ".env"
        environment_file.write_text(
            self.deployed_env_text(release_sha, kyc_gate=kyc_gate)
        )
        module = _load_preparer()
        result = module.prepare(
            environment_file,
            release_sha,
            self.checkout / ".runtime-rust",
            ROOT,
            MIGRATION_TARGET,
        )
        return result["runtime_env"]

    def dry_run(self, **overrides) -> subprocess.CompletedProcess[str]:
        environment = self.dry_run_environment()
        environment.update(overrides)
        return subprocess.run(
            ["bash", "scripts/deploy.sh", "--dry-run"],
            cwd=self.checkout,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )

    # -- observations ------------------------------------------------------
    @property
    def ledger(self) -> list[str]:
        if not self.log_path.exists():
            return []
        return self.log_path.read_text().splitlines()

    def state_text(self) -> str:
        state = self.tmp_path / "runtime-deploy-state/staging.state"
        return state.read_text() if state.exists() else ""

    def stopped_services(self) -> set[str]:
        stopped = set()
        for command in self.ledger:
            if command.startswith("sudo:systemctl stop "):
                stopped.update(command.split()[2:])
        return stopped


@pytest.fixture()
def release_fixture(tmp_path):
    return RustReleaseFixture(tmp_path)


def test_fixture_checkout_relies_on_the_documented_layout_override(release_fixture):
    """The fixture checkout is not `<canonical>/be`, so the override applies.

    `scripts/deploy.sh` cannot infer the environment from this path and then
    demands all four explicit values; the fixture supplies them, which is what
    keeps a non-canonical (fake-tool-preserving) checkout deployable.
    """
    deploy = (ROOT / "scripts" / "deploy.sh").read_text()
    environment = release_fixture.deploy_environment(release_fixture.old_sha)
    for name in ("TARGET_BRANCH", "SERVICE_NAME", "HEALTH_URL", "DEPLOY_ENVIRONMENT"):
        assert f"${{{name}:?" in deploy, name
        assert environment.get(name), name
    assert not str(release_fixture.checkout).endswith(("/prod/be", "/staging/be"))


def test_python_only_deploy_matches_the_pre_rust_command_ledger(release_fixture):
    """PM requirement 1: no Rust unit, no artifact — the exact old commands."""
    new_sha = release_fixture.push_rust_release(enabled=False)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    executed = release_fixture.ledger
    # Every golden entry appears, in relative order.
    positions = [
        next(i for i, line in enumerate(executed) if entry in line)
        for entry in GOLDEN_PYTHON_LEDGER
    ]
    assert positions == sorted(positions)
    # No Rust-side command of any kind ran.
    assert not any("prepare_rust_runtime" in entry for entry in executed)
    assert not any("prepare_rust_budget" in entry for entry in executed)
    assert not any("rust" in entry.lower() for entry in executed)
    assert not any("verdaxis-rust" in entry for entry in executed)
    # Exactly one service restart, exactly as before.
    restarts = [e for e in executed if e.startswith("sudo:systemctl restart")]
    assert restarts == [
        "sudo:systemctl restart verdaxis-backend-staging-test.service"
    ]


def test_dry_run_never_executes_build_code(release_fixture):
    release_fixture.build_artifact("f" * 40)
    candidate = release_fixture.push_new_release()
    result = release_fixture.dry_run()
    assert result.returncode == 0, result.stderr
    assert f"APPROVED_RELEASE_SHA={candidate}" in result.stdout
    executed = release_fixture.ledger
    assert not any("python:" in entry for entry in executed)
    assert not any("alembic:" in entry for entry in executed)
    assert not any("rust-binary:" in entry for entry in executed)
    assert "runtime compatibility remains untested" in result.stdout


def test_both_health_results_are_required(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, DEPLOY_RUST_UNREADY="1")
    assert result.returncode != 0
    assert "DEPLOYMENT_STATE=blocked" in release_fixture.state_text()
    assert release_fixture.stopped_services() == {
        "verdaxis-backend-staging-test.service",
        "verdaxis-rust-staging.service",
    }
    rust_runtime = next(
        e for e in release_fixture.ledger if "prepare_rust_runtime.py" in e
    )


def test_rust_deploy_restarts_both_services_and_clears_state(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    restarts = [e for e in release_fixture.ledger if e.startswith("sudo:systemctl restart")]
    assert restarts == [
        "sudo:systemctl restart verdaxis-backend-staging-test.service verdaxis-rust-staging.service"
    ]
    assert release_fixture.state_text() == ""
    runtime_env = release_fixture.checkout / ".runtime-rust/runtime.env"
    assert runtime_env.is_file()
    assert runtime_env.stat().st_mode & 0o777 == 0o600
    assert "MIGRATOR" not in runtime_env.read_text()
    active = release_fixture.checkout / ".runtime-rust/verdaxis-read"
    assert active.is_file() and not active.is_symlink()
    budget = release_fixture.checkout / ".runtime-rust/budget.json"
    assert budget.is_file()


def test_rust_health_uses_loopback_identity_url(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    release_fixture.deploy(new_sha)
    identity = next(
        e for e in release_fixture.ledger if e.startswith("curl:") and "8011" in e
    )
    assert "http://127.0.0.1:8011/health/ready" in identity


@pytest.mark.parametrize("load_state", ["not-found", "masked", "error"])
def test_unavailable_rust_unit_refuses_before_source_or_restart(release_fixture, load_state):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, DEPLOY_RUST_LOAD_STATE=load_state)
    assert result.returncode != 0
    assert "unit" in (result.stderr + result.stdout).lower()
    assert not any("sudo:systemctl" in e for e in release_fixture.ledger)
    assert release_fixture.state_text() == ""
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == release_fixture.old_sha


def test_missing_provenance_record_refuses_before_any_stop(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, RUST_PROVENANCE_RECORD="")
    assert result.returncode != 0
    assert "provenance" in (result.stderr + result.stdout).lower()
    assert not release_fixture.stopped_services()
    assert not any("sudo:systemctl" in e for e in release_fixture.ledger)
    assert release_fixture.state_text() == ""
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == release_fixture.old_sha


def test_missing_signature_refuses_before_any_stop(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, RUST_PROVENANCE_SIGNATURE="")
    assert result.returncode != 0
    assert not release_fixture.stopped_services()


def test_missing_record_and_signature_refuses_before_any_stop(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, RUST_PROVENANCE_RECORD="", RUST_PROVENANCE_SIGNATURE="")
    assert result.returncode != 0
    assert "provenance" in (result.stderr + result.stdout).lower()
    assert not release_fixture.stopped_services()
    assert release_fixture.state_text() == ""


def test_divergent_host_key_refuses_before_any_stop(release_fixture):
    """PM requirement 2: /etc/verdaxis key digest must equal the pinned digest."""
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    divergent = release_fixture.tmp_path / "divergent.pub.pem"
    divergent.write_bytes(b"-----BEGIN PUBLIC KEY-----\nnot the pinned key\n-----END PUBLIC KEY-----\n")
    result = release_fixture.deploy(new_sha, HOST_PROVENANCE_KEY_PATH=str(divergent))
    assert result.returncode != 0
    assert "provenance public key" in (result.stderr + result.stdout)
    assert not release_fixture.stopped_services()
    assert release_fixture.state_text() == ""


def test_host_key_matching_the_pinned_digest_is_preferred(release_fixture):
    """An /etc/verdaxis key equal to the reviewed digest is used as-is."""
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    host_key = release_fixture.tmp_path / "etc-verdaxis.pub.pem"
    host_key.write_bytes(release_fixture.public_key)
    result = release_fixture.deploy(new_sha, HOST_PROVENANCE_KEY_PATH=str(host_key))
    assert result.returncode == 0, result.stderr
    verify = next(e for e in release_fixture.ledger if "--provenance-public-key" in e)
    assert str(host_key) in verify


def test_missing_host_key_file_falls_back_to_tree_key(release_fixture):
    """An absent /etc/verdaxis key passes the reviewed tree's file instead."""
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    verify = next(
        e for e in release_fixture.ledger if "--provenance-public-key" in e
    )
    # The tree copy inside the selected checkout was used.
    assert "--provenance-public-key" in verify
    assert str(release_fixture.checkout / PROVENANCE_KEY) in verify or (
        "deploy/rust/artifact-provenance.pub.pem" in verify
    )


def test_tampered_archive_refuses(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    archive = release_fixture.artifact_root / "release.tar"
    archive.write_bytes(archive.read_bytes()[:-8] + b"tampered")
    result = release_fixture.deploy(new_sha)
    assert result.returncode != 0
    assert not release_fixture.stopped_services()
    assert release_fixture.state_text() == ""


def test_wrong_source_sha_record_refuses(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact("e" * 40)
    result = release_fixture.deploy(new_sha)
    assert result.returncode != 0
    assert not release_fixture.stopped_services()


def test_failed_signature_verification_refuses(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    signature = release_fixture.artifact_root / "provenance-record.sig"
    signature.write_bytes(b"not a valid signature")
    result = release_fixture.deploy(new_sha)
    assert result.returncode != 0
    assert not release_fixture.stopped_services()
    assert release_fixture.state_text() == ""


def test_rust_failure_keeps_selected_code_identity_and_blocked_state(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, fail_phase="rust-runtime")
    assert result.returncode != 0
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == new_sha
    release_env = release_fixture.checkout / ".runtime-release.env"
    assert release_env.read_text() == (
        f"ENVIRONMENT=staging\nRELEASE_SHA={new_sha}\n"
        f"MIGRATION_REVISION={MIGRATION_TARGET}\n"
    )
    assert "DEPLOYMENT_STATE=blocked" in release_fixture.state_text()


def test_changed_budget_refuses_and_blocks(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, fail_phase="budget")
    assert result.returncode != 0
    assert "DEPLOYMENT_STATE=blocked" in release_fixture.state_text()
    assert release_fixture.stopped_services() == set()


def test_stale_authorization_does_not_authorize_a_unit_start(release_fixture):
    """A leftover restart-authorized state denies unit starts until cleared.

    The deploy transaction itself may run a corrected forward release (the
    locks were released); the start guard is what fail-closes the window, so
    that is what this test pins: a stale authorization from another SHA never
    authorizes a backend start.
    """
    state_dir = release_fixture.checkout / ".runtime-deploy"
    state_dir.mkdir(parents=True, exist_ok=True)
    (state_dir / "staging.state").write_text(
        "DEPLOYMENT_STATE=restart-authorized\n"
        f"ENVIRONMENT=staging\nRELEASE_SHA={release_fixture.old_sha}\n"
    )
    denied = subprocess.run(
        [
            "/usr/bin/env", "-i", "/usr/bin/python3", "-I", "-S", "-B",
            str(release_fixture.checkout / DEPLOY_START_GUARD),
            "--environment", "staging",
            "--expected-release-sha", "c" * 40,
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert denied.returncode != 0
    assert denied.stdout == ""


def test_wrong_revision_refuses_before_source_mutation(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    environment = release_fixture.deploy_environment(new_sha)
    environment["MIGRATION_TARGET_REVISION"] = "not_a_checkpoint"
    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=release_fixture.checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == release_fixture.old_sha


def test_moved_sha_refuses_before_source_mutation(release_fixture):
    release_fixture.build_artifact("f" * 40)
    approved = release_fixture.push_new_release()
    release_fixture.push_new_release()
    result = release_fixture.deploy(approved)
    assert result.returncode != 0
    assert "moved after approval" in result.stderr
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == release_fixture.old_sha


@pytest.mark.parametrize("phase", ["preflight", "budget", "rust-runtime", "health"])
def test_failures_retain_blocked_state_and_stop_attempted_services(release_fixture, phase):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, fail_phase=phase)
    assert result.returncode != 0
    assert "DEPLOYMENT_STATE=blocked" in release_fixture.state_text()
    # Only a restart-attempted deploy stops services.
    if phase == "health":
        assert release_fixture.stopped_services() == {
            "verdaxis-backend-staging-test.service",
            "verdaxis-rust-staging.service",
        }


def test_interrupted_restart_window_releases_locks_and_keeps_state(release_fixture):
    """SIGKILL mid-restart keeps restart-authorized state but frees both locks."""
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    release_fixture._write_deployed_env(new_sha)
    environment = release_fixture.deploy_environment(new_sha)
    # The start guard reads the checkout's own .runtime-deploy, so the deploy
    # must write its state there for the guard to see it (as on the host).
    environment["DEPLOY_STATE_DIR"] = str(release_fixture.checkout / ".runtime-deploy")
    environment["DEPLOY_BLOCK_PHASE"] = "scripts/validate_health_response.py"
    running = subprocess.Popen(
        ["bash", "scripts/deploy.sh"],
        cwd=release_fixture.checkout,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    state_dir = release_fixture.checkout / ".runtime-deploy"
    try:
        for _ in range(250):
            state = (state_dir / "staging.state")
            if state.exists() and "readiness-pending" in state.read_text():
                break
            time.sleep(0.02)
        assert "DEPLOYMENT_STATE=readiness-pending" in (state_dir / "staging.state").read_text()
        restarts = [
            e for e in release_fixture.ledger if e.startswith("sudo:systemctl restart")
        ]
        assert restarts == [
            "sudo:systemctl restart verdaxis-backend-staging-test.service verdaxis-rust-staging.service"
        ]
        os.kill(running.pid, signal.SIGKILL)
        running.wait(timeout=5)
        # State is retained fail-closed; the locks were released by the kernel.
        assert "DEPLOYMENT_STATE=readiness-pending" in (state_dir / "staging.state").read_text()
    finally:
        if running.poll() is None:
            os.kill(running.pid, signal.SIGKILL)
            running.wait(timeout=5)
        try:
            os.killpg(running.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    # A later start guard refuses the stale state.
    denied = subprocess.run(
        [
            "/usr/bin/env", "-i", "/usr/bin/python3", "-I", "-S", "-B",
            str(release_fixture.checkout / DEPLOY_START_GUARD),
            "--environment", "staging",
            "--expected-release-sha", new_sha,
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    assert denied.returncode != 0


def test_concurrent_deploy_refuses_while_lock_held(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    release_fixture._write_deployed_env(new_sha)
    environment = release_fixture.deploy_environment(new_sha)
    environment["DEPLOY_BLOCK_PHASE"] = "scripts/preflight_runtime.py"
    running = subprocess.Popen(
        ["bash", "scripts/deploy.sh"],
        cwd=release_fixture.checkout,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    try:
        for _ in range(250):
            if ledger_contains(release_fixture.ledger, "python:scripts/preflight_runtime.py"):
                break
            time.sleep(0.02)
        assert ledger_contains(release_fixture.ledger, "python:scripts/preflight_runtime.py")
        concurrent = subprocess.run(
            ["bash", "scripts/deploy.sh"],
            cwd=release_fixture.checkout,
            env={**environment, "DEPLOY_BLOCK_PHASE": ""},
            capture_output=True,
            text=True,
            check=False,
        )
        assert concurrent.returncode != 0
        assert "already running" in concurrent.stderr
    finally:
        os.killpg(running.pid, signal.SIGTERM)
        running.wait(timeout=10)


def test_runtime_environment_carries_only_the_projected_settings(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    keys = {
        line.split("=", 1)[0]
        for line in (release_fixture.checkout / ".runtime-rust/runtime.env").read_text().splitlines()
        if "=" in line
    }
    assert keys == {
        "ENVIRONMENT",
        "RELEASE_SHA",
        "MIGRATION_REVISION",
        "DATABASE_URL",
        "DB_SESSION_TIME_ZONE",
        "READ_ADMISSION_SLOTS",
        "KYC_EXECUTION_GATE_ENABLED",
    }
    # The shipped fixture `.env` carries the Private Beta switch state, so the
    # projection is proven to follow the deployed file rather than a default.
    assert (
        (release_fixture.checkout / ".runtime-rust/runtime.env").read_text()
        .splitlines()
    ) == sorted(
        [
            'DATABASE_URL="postgresql://verdaxis_app_staging:fixture-password'
            '@127.0.0.1:5432/verdaxis_staging"',
            "DB_SESSION_TIME_ZONE=UTC",
            "ENVIRONMENT=staging",
            "KYC_EXECUTION_GATE_ENABLED=false",
            f"MIGRATION_REVISION={MIGRATION_TARGET}",
            "READ_ADMISSION_SLOTS=16",
            f"RELEASE_SHA={new_sha}",
        ]
    )


@pytest.mark.parametrize(
    ("declared", "expected"),
    [(None, "true"), ("false", "false"), ("TRUE", "true"), ("off", "false")],
)
def test_runtime_environment_resolves_the_kyc_switch_like_python(
    release_fixture, declared, expected
):
    """The Rust reads bind Python's switch, so the deploy must project it."""
    runtime_env = release_fixture.prepare_runtime_environment(
        release_fixture.old_sha, kyc_gate=declared
    )
    assert f"KYC_EXECUTION_GATE_ENABLED={expected}\n" in runtime_env.read_text()


def test_runtime_environment_refuses_an_ambiguous_kyc_switch(release_fixture):
    with pytest.raises(_load_preparer().RustRuntimeError, match="KYC_EXECUTION_GATE_ENABLED"):
        release_fixture.prepare_runtime_environment(
            release_fixture.old_sha, kyc_gate="maybe"
        )


def test_no_rust_unit_manifest_entry_disables_rust_even_if_policy_enabled(release_fixture):
    """The committed manifest is the second activation record (Task 12c)."""
    source = release_fixture.source
    manifest = source / UNIT_MANIFEST
    manifest.write_text("staging\tverdaxis-backend-staging.service\n")
    _git(source, "add", "-A")
    _git(source, "commit", "-qm", "manifest without rust unit")
    _git(source, "push", "-q", "origin", "staging")
    new_sha = _git(source, "rev-parse", "HEAD").stdout.strip()
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    assert not any("prepare_rust_runtime" in e for e in release_fixture.ledger)
    assert not any("verdaxis-rust" in e for e in release_fixture.ledger)


def test_golden_python_ledger_is_recorded_completely(release_fixture):
    """Guard the golden list itself: every entry must appear in a Python-only run."""
    new_sha = release_fixture.push_rust_release(enabled=False)
    result = release_fixture.deploy(new_sha)
    assert result.returncode == 0, result.stderr
    for entry in GOLDEN_PYTHON_LEDGER:
        assert ledger_contains(release_fixture.ledger, entry), f"missing {entry}"
