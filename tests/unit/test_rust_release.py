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
            "FRONTEND_URL=https://staging.verdaxis.exchange\n"
            "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\n"
            "RESEND_API_KEY=re_fixture_key\n"
            "OPENAI_API_KEY=sk-unrelated\n"
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
if [[ "${1:-}" == "scripts/validate_health_response.py" ]]; then
    # deploy.sh pipes the readiness payload into the real helper, which reads
    # it from stdin. A wrapper that exits without draining that pipe can lose
    # the race with the `printf` writer, whose SIGPIPE (141) then fails the
    # pipeline and the health gate although the payload was valid.
    cat > /dev/null
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
if [[ "${DEPLOY_BLOCK_PHASE:-}" == "${1:-}" ]]; then
    printf '%s\\n' "$1" > "${DEPLOY_TEST_LOG:?}.blocked"
    sleep 120
fi
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
if [[ "${1:-}" == "-n" && "${2:-}" != "-l" ]]; then shift; fi
printf 'sudo:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "${1:-}" == "-n" && "${2:-}" == "-l" ]]; then
    if [[ -n "${DEPLOY_SUDO_DENY_ACTION:-}" && "${3:-}" == "systemctl" && "${4:-}" == "${DEPLOY_SUDO_DENY_ACTION}" ]]; then exit 1; fi
    test "${DEPLOY_SUDO_DENY:-}" != 1
    exit $?
fi
if [[ "${DEPLOY_RUST_START_FAIL:-}" == 1 && "$*" == "systemctl restart verdaxis-rust-staging.service" ]]; then exit 50; fi
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
if [[ "${DEPLOY_RUST_UNREADY:-}" == "1" && "$*" == *8011* ]]; then
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
        self.artifact_root = self.tmp_path / f"rust-artifact-{release_sha}"
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
            **{
                key: value for key, value in os.environ.items()
                if not key.startswith(('DEPLOY_', 'RUST_', 'HEALTH_', 'MIGRATION_'))
                and key not in {'APPROVED_RELEASE_SHA', 'HOST_PROVENANCE_KEY_PATH'}
            },
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

    def deploy(self, release_sha: str, *, fail_phase: str = "", cache_listener: str | None = None, **overrides) -> subprocess.CompletedProcess[str]:
        self._write_deployed_env(release_sha)
        if cache_listener is not None:
            with (self.checkout / ".env").open("a") as environment_file:
                environment_file.write(f"RUST_CACHE_LISTENER={cache_listener}\n")
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
            "FRONTEND_URL=https://staging.verdaxis.exchange\n"
            "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\n"
            "RESEND_API_KEY=re_fixture_key\n"
            "OPENAI_API_KEY=sk-unrelated\n"
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
        self, release_sha: str, *, kyc_gate: str | None = "false",
        previous: str | None = None, omit: str | None = None,
    ) -> Path:
        """Run the real Rust runtime preparation over the deployed `.env`."""
        environment_file = self.checkout / ".env"
        body = self.deployed_env_text(release_sha, kyc_gate=kyc_gate)
        if previous is not None:
            body += f"JWT_SECRET_PREVIOUS={previous}\n"
        if omit is not None:
            body = "".join(
                line for line in body.splitlines(keepends=True)
                if not line.startswith(f"{omit}=")
            )
        environment_file.write_text(body)
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


@pytest.mark.parametrize("fail_phase, expected_exit", [("", 0), ("health", 44)])
def test_fixture_health_validator_drains_its_input(release_fixture, fail_phase, expected_exit):
    """A payload larger than the pipe capacity exposes an early reader exit."""
    environment = release_fixture.deploy_environment(release_fixture.old_sha)
    environment["DEPLOY_FAIL_PHASE"] = fail_phase
    result = subprocess.run(
        [
            "bash", "-c",
            'set -o pipefail; cat | "$1" scripts/validate_health_response.py; '
            'statuses=("${PIPESTATUS[@]}"); '
            'printf "PIPESTATUS=%s %s\\n" "${statuses[@]}" >&2; '
            'if (( statuses[0] != 0 )); then exit "${statuses[0]}"; fi; '
            'exit "${statuses[1]}"',
            "fixture-health-pipeline", str(release_fixture.checkout / "venv/bin/python"),
        ],
        input=b"x" * (1024 * 1024),
        env=environment,
        capture_output=True,
        check=False,
        timeout=10,
    )
    assert result.returncode == expected_exit, result.stderr.decode()
    assert f"PIPESTATUS=0 {expected_exit}" in result.stderr.decode()
    assert release_fixture.ledger == ["python:scripts/validate_health_response.py"]


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


@pytest.mark.parametrize(
    "failure",
    [
        {"fail_phase": "health"},
        {"fail_phase": "rust-runtime"},
        {"fail_phase": "budget"},
        {"DEPLOY_RUST_START_FAIL": "1"},
        {"DEPLOY_RUST_UNREADY": "1"},
    ],
    ids=["backend-health", "rust-runtime", "budget", "rust-start", "rust-readiness"],
)
def test_rust_activation_failure_never_enables_the_unit(release_fixture, failure):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, **failure)
    assert result.returncode != 0
    assert not any(
        e == "sudo:systemctl enable verdaxis-rust-staging.service"
        for e in release_fixture.ledger
    )


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


def _budget_line(ledger: list[str]) -> str:
    return next(e for e in ledger if "prepare_rust_budget.py" in e and "--verify" not in e)


_BUDGET_MODULE = None


def _budget_module():
    """The one loaded instance, like `_load_preparer` above."""
    global _BUDGET_MODULE
    if _BUDGET_MODULE is None:
        spec = importlib.util.spec_from_file_location(
            "prepare_rust_budget", ROOT / "scripts" / "prepare_rust_budget.py"
        )
        module = importlib.util.module_from_spec(spec)
        assert spec.loader is not None
        spec.loader.exec_module(module)
        _BUDGET_MODULE = module
    return _BUDGET_MODULE


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


def test_runtime_environment_projects_the_phase2_private_settings(release_fixture):
    """Task 23: the six groups need one verification and invite authority."""
    runtime_env = release_fixture.prepare_runtime_environment(
        release_fixture.old_sha, previous="p" * 40
    )
    body = runtime_env.read_text()
    for line in (
        "JWT_SECRET=" + "k" * 40,
        "JWT_SECRET_PREVIOUS=" + "p" * 40,
        "JWT_ISSUER=verdaxis-staging-api",
        "JWT_AUDIENCE=verdaxis-staging-web",
        'FRONTEND_URL="https://staging.verdaxis.exchange"',
        'EMAIL_FROM="Verdaxis <noreply@verdaxis.exchange>"',
        "RESEND_API_KEY=re_fixture_key",
    ):
        assert line + "\n" in body
    # The migrator and an unrelated provider key never reach the long-running
    # process; only the invite provider is projected.
    assert "MIGRATOR" not in body
    assert "OPENAI_API_KEY" not in body
    assert runtime_env.stat().st_mode & 0o777 == 0o600
    assert runtime_env.stat().st_uid == os.geteuid()


def test_runtime_environment_blank_previous_key_is_absent(release_fixture):
    """No rotation in progress means no previous key, not an empty one."""
    body = release_fixture.prepare_runtime_environment(
        release_fixture.old_sha, previous="   "
    ).read_text()
    assert "JWT_SECRET_PREVIOUS" not in body


def test_runtime_environment_refuses_a_missing_or_short_phase2_key(release_fixture):
    module = _load_preparer()
    with pytest.raises(module.RustRuntimeError, match="RESEND_API_KEY") as error:
        release_fixture.prepare_runtime_environment(
            release_fixture.old_sha, omit="RESEND_API_KEY"
        )
    # The refusal names the key, never its value.
    assert "re_fixture_key" not in str(error.value)
    with pytest.raises(module.RustRuntimeError, match="JWT_SECRET_PREVIOUS"):
        release_fixture.prepare_runtime_environment(
            release_fixture.old_sha, previous="p" * 20
        )


def test_phase2_projection_refuses_a_partial_or_unusable_value():
    """"all or nothing" and URL/secret checks live in the projection itself."""
    module = _load_preparer()
    values = {
        "JWT_SECRET": "k" * 40,
        "JWT_ISSUER": "verdaxis-staging-api",
        "JWT_AUDIENCE": "verdaxis-staging-web",
        "FRONTEND_URL": "https://staging.verdaxis.exchange",
        "EMAIL_FROM": "Verdaxis <noreply@verdaxis.exchange>",
        "RESEND_API_KEY": "re_fixture_key",
    }
    assert set(module.phase2_settings(values)) == set(module.PHASE2_REQUIRED_KEYS)
    for key in module.PHASE2_REQUIRED_KEYS:
        broken = dict(values)
        broken.pop(key)
        with pytest.raises(module.RustRuntimeError, match=key):
            module.phase2_settings(broken)
    for url in ("", "staging.verdaxis.exchange", "ftp://example.test", "https://a b"):
        with pytest.raises(module.RustRuntimeError, match="FRONTEND_URL"):
            module.phase2_settings({**values, "FRONTEND_URL": url})
    with pytest.raises(module.RustRuntimeError, match="at least 32"):
        module.phase2_settings({**values, "JWT_SECRET": "short"})


def test_runtime_environment_never_projects_the_watchlist_activation(tmp_path):
    """PD5: only the release metadata records the watchlist producer activation.

    A value in the deployed `.env` is not release metadata, so the private
    runtime file never carries it and Rust cannot read it from there.
    """
    module = _load_preparer()
    release_sha = "a" * 40
    environment_file = tmp_path / ".env"
    environment_file.write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={release_sha}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging:fixture-secret"
        "@127.0.0.1:5432/verdaxis_staging\n"
        "JWT_SECRET=" + "k" * 40 + "\n"
        "JWT_ISSUER=verdaxis-staging-api\n"
        "JWT_AUDIENCE=verdaxis-staging-web\n"
        "FRONTEND_URL=https://staging.verdaxis.exchange\n"
        "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\n"
        "RESEND_API_KEY=re_fixture_key\n"
        "WATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-01T00:00:00+00:00\n"
    )
    result = module.prepare(
        environment_file, release_sha, tmp_path / "runtime", ROOT, MIGRATION_TARGET
    )
    assert "WATCHLIST_PRODUCER_ACTIVATED_AT" not in result["runtime_env"].read_text()


def _market_environment_file(tmp_path: Path, extra: str) -> tuple[Path, str]:
    release_sha = "a" * 40
    environment_file = tmp_path / ".env"
    environment_file.write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={release_sha}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app_staging:fixture-secret"
        "@127.0.0.1:5432/verdaxis_staging\n"
        "JWT_SECRET=" + "k" * 40 + "\n"
        "JWT_ISSUER=verdaxis-staging-api\n"
        "JWT_AUDIENCE=verdaxis-staging-web\n"
        "FRONTEND_URL=https://staging.verdaxis.exchange\n"
        "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\n"
        "RESEND_API_KEY=re_fixture_key\n" + extra
    )
    return environment_file, release_sha


def test_runtime_projection_carries_market_settings_with_python_defaults(tmp_path):
    """The seven market keys reach the Rust runtime file with Python's rules.

    An absent key is not projected, so the Rust service applies Python's
    default; a present key is validated as Python validates it and projected
    in normal form. The watchlist producer keys are never projected.
    """
    module = _load_preparer()
    environment_file, release_sha = _market_environment_file(tmp_path, "")
    result = module.prepare(
        environment_file, release_sha, tmp_path / "absent", ROOT, MIGRATION_TARGET
    )
    projected = result["runtime_env"].read_text()
    for key in (
        "AUTO_MATCHING_ENABLED",
        "MARKET_SUPPORT_ENABLED",
        "MARKET_SUPPORT_MAX_TTL_HOURS",
        "MARKET_SUPPORT_CONTEXT_TTL_MINUTES",
        "MARKET_SUPPORT_CONSENT_VERSION",
        "MARKET_SUPPORT_CONSENT_REFERENCE",
        "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS",
    ):
        assert f"{key}=" not in projected, key

    environment_file, release_sha = _market_environment_file(
        tmp_path,
        "AUTO_MATCHING_ENABLED=Off\n"
        "MARKET_SUPPORT_ENABLED=yes\n"
        "MARKET_SUPPORT_MAX_TTL_HOURS= 72 \n"
        "MARKET_SUPPORT_CONTEXT_TTL_MINUTES=30\n"
        "MARKET_SUPPORT_CONSENT_VERSION=2026-10-01\n"
        "MARKET_SUPPORT_CONSENT_REFERENCE=Staging support terms\n"
        "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS=00000000-0000-0000-0000-000000003001\n"
        "WATCHLIST_EVENT_PRODUCER=rust-market-v1\n"
        "WATCHLIST_PRODUCER_ACTIVATED_AT=2026-11-01T00:00:00+00:00\n",
    )
    result = module.prepare(
        environment_file, release_sha, tmp_path / "present", ROOT, MIGRATION_TARGET
    )
    projected = result["runtime_env"].read_text().splitlines()
    for line in (
        "AUTO_MATCHING_ENABLED=false",
        "MARKET_SUPPORT_ENABLED=true",
        "MARKET_SUPPORT_MAX_TTL_HOURS=72",
        "MARKET_SUPPORT_CONTEXT_TTL_MINUTES=30",
        "MARKET_SUPPORT_CONSENT_VERSION=2026-10-01",
        'MARKET_SUPPORT_CONSENT_REFERENCE="Staging support terms"',
        "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS=00000000-0000-0000-0000-000000003001",
    ):
        assert line in projected, line
    assert not any(line.startswith("WATCHLIST_") for line in projected)

    for extra, reason in (
        ("AUTO_MATCHING_ENABLED=maybe\n", "AUTO_MATCHING_ENABLED"),
        ("MARKET_SUPPORT_ENABLED=2\n", "MARKET_SUPPORT_ENABLED"),
        ("MARKET_SUPPORT_MAX_TTL_HOURS=0\n", "MARKET_SUPPORT_MAX_TTL_HOURS"),
        ("MARKET_SUPPORT_MAX_TTL_HOURS=721\n", "MARKET_SUPPORT_MAX_TTL_HOURS"),
        ("MARKET_SUPPORT_CONTEXT_TTL_MINUTES=1441\n", "MARKET_SUPPORT_CONTEXT_TTL_MINUTES"),
        ("MARKET_SUPPORT_CONTEXT_TTL_MINUTES=ten\n", "MARKET_SUPPORT_CONTEXT_TTL_MINUTES"),
        ("MARKET_SUPPORT_ENABLED=true\n", "explicitly configured"),
        (
            "MARKET_SUPPORT_ENABLED=true\nMARKET_SUPPORT_CONSENT_VERSION=\n"
            "MARKET_SUPPORT_CONSENT_REFERENCE=terms\n",
            "required when enabled",
        ),
    ):
        environment_file, release_sha = _market_environment_file(tmp_path, extra)
        with pytest.raises(module.RustRuntimeError, match=reason):
            module.prepare(
                environment_file, release_sha, tmp_path / "refused", ROOT, MIGRATION_TARGET
            )


def test_runtime_environment_preserves_quoted_secret_bytes(tmp_path):
    module = _load_preparer()
    values = {
        "JWT_SECRET": '  current\\key"' + "k" * 40 + "  ",
        "JWT_SECRET_PREVIOUS": ' previous\\key"' + "p" * 40 + " ",
        "JWT_ISSUER": " issuer ",
        "JWT_AUDIENCE": " audience ",
        "FRONTEND_URL": "https://example.test/path/",
        "EMAIL_FROM": '"Verdaxis Support" <sender@example.test>',
        "RESEND_API_KEY": "re_fixture_key",
    }
    projected = module.phase2_settings(values)
    assert projected == values
    body = module.write_env_file(tmp_path / "runtime", projected).read_text()
    for key, value in values.items():
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        assert f'{key}="{escaped}"\n' in body or f"{key}={value}\n" in body


@pytest.mark.parametrize("boundary", ["source", "directory", "target"])
def test_runtime_environment_refuses_symlink_boundaries(tmp_path, boundary):
    module = _load_preparer()
    source = tmp_path / "source.env"
    source.write_text("do not read through a link")
    output = tmp_path / "runtime"
    output.mkdir()
    if boundary == "source":
        link = tmp_path / "linked.env"
        link.symlink_to(source)
        with pytest.raises(module.RustRuntimeError, match="regular file"):
            module.read_regular(link)
    elif boundary == "directory":
        link = tmp_path / "linked-runtime"
        link.symlink_to(output, target_is_directory=True)
        with pytest.raises(module.RustRuntimeError, match="real directory"):
            module.write_env_file(link, {"JWT_SECRET": "k" * 40})
    else:
        (output / "runtime.env").symlink_to(source)
        with pytest.raises(module.RustRuntimeError, match="symlink"):
            module.write_env_file(output, {"JWT_SECRET": "k" * 40})
    assert source.read_text() == "do not read through a link"


def test_sudo_denial_refuses_before_source_state_or_restart(release_fixture):
    new_sha = release_fixture.push_rust_release(enabled=True)
    release_fixture.build_artifact(new_sha)
    result = release_fixture.deploy(new_sha, DEPLOY_SUDO_DENY="1")
    assert result.returncode != 0
    assert _git(release_fixture.checkout, "rev-parse", "HEAD").stdout.strip() == release_fixture.old_sha
    assert release_fixture.state_text() == ""
    assert not release_fixture.stopped_services()

