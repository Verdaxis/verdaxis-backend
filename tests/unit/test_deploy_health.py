"""Strict release-health and operator installation gates."""

import hashlib
import io
import importlib.util
import json
import os
import re
import shutil
import signal
import subprocess
import tarfile
from pathlib import Path

import pytest

from scripts.validate_health_response import HealthResponseError, validate_health_response


ROOT = Path(__file__).parents[2]
SHA = "a" * 40
MIGRATION_EXPECTED = "pa_20260715_analytics_facts"
MIGRATION_TARGET = "rh_20260720_runtime_metadata"
UNIT_MANIFEST = "deploy/systemd/runtime-units.manifest"
SYSTEMD_UNITS = {
    "production": (
        "deploy/systemd/verdaxis-backend.service",
        "deploy/systemd/verdaxis-news-refresh.service",
        "deploy/systemd/verdaxis-news-refresh.timer",
        "deploy/systemd/verdaxis-product-analytics-prune.service",
        "deploy/systemd/verdaxis-product-analytics-prune.timer",
    ),
    "staging": (
        "deploy/systemd/verdaxis-backend-staging.service",
        "deploy/systemd/verdaxis-news-refresh-staging.service",
        "deploy/systemd/verdaxis-news-refresh-staging.timer",
        "deploy/systemd/verdaxis-product-analytics-prune-staging.service",
        "deploy/systemd/verdaxis-product-analytics-prune-staging.timer",
    ),
}


def test_health_response_requires_exact_status_environment_and_release_sha():
    validate_health_response(
        json.dumps(
            {
                "status": "ok",
                "db": "ok",
                "environment": "production",
                "release_sha": SHA,
            }
        ),
        expected_environment="production",
        expected_release_sha=SHA,
    )

    for mutation in (
        {"status": "okay"},
        {"db": "connected"},
        {"environment": "staging"},
        {"release_sha": "b" * 40},
    ):
        payload = {
            "status": "ok",
            "db": "ok",
            "environment": "production",
            "release_sha": SHA,
            **mutation,
        }
        with pytest.raises(HealthResponseError):
            validate_health_response(
                json.dumps(payload),
                expected_environment="production",
                expected_release_sha=SHA,
            )


@pytest.mark.parametrize("payload", ["", "not-json", "[]", "null"])
def test_health_response_rejects_malformed_or_non_object_json(payload):
    with pytest.raises(HealthResponseError):
        validate_health_response(
            payload,
            expected_environment="production",
            expected_release_sha=SHA,
        )


def test_deploy_rejects_dirty_release_and_uses_json_health_gate():
    source = (ROOT / "scripts/deploy.sh").read_text()

    assert "ALLOW_DIRTY" not in source
    assert "status --porcelain" in source
    assert "validate_health_response.py" in source
    assert "grep -q" not in source
    assert source.rindex("status --porcelain") < source.index(
        'write_release_artifact "$CURRENT_SHA"'
    )
    assert "APPROVED_RELEASE_SHA" in source
    assert "archive --format=tar" in source
    assert "clone" not in source
    assert "ln -s" not in source
    assert source.index('assert_release_tree_regular "$CURRENT_SHA"') < source.index(
        'write_release_artifact "$CURRENT_SHA"'
    )


def _git(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def _write_executable(path: Path, source: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(source)
    path.chmod(0o755)


def _deployment_checkout(tmp_path: Path) -> tuple[Path, Path, str]:
    origin = tmp_path / "origin.git"
    source = tmp_path / "approved-source"
    checkout = tmp_path / "deploy-checkout"
    origin.mkdir()
    source.mkdir()
    _git(origin, "init", "--bare", "-q")
    _git(source, "init", "-q", "-b", "staging")
    _git(source, "config", "user.email", "runtime-test@example.invalid")
    _git(source, "config", "user.name", "Runtime Test")

    scripts = source / "scripts"
    scripts.mkdir()
    shutil.copy2(ROOT / "scripts/deploy.sh", scripts / "deploy.sh")
    shutil.copy2(
        ROOT / "scripts/converge_runtime_acls.py",
        scripts / "converge_runtime_acls.py",
    )
    (scripts / "preflight_runtime.py").write_text("# deployment test preflight\n")
    for relative in SYSTEMD_UNITS["staging"]:
        destination = source / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        unit_text = (ROOT / relative).read_text()
        unit_text = re.sub(
            r"^ExecStart=.*$", "ExecStart=/bin/true", unit_text, flags=re.MULTILINE
        )
        destination.write_text(unit_text)
    (source / UNIT_MANIFEST).write_text(
        "".join(
            f"staging\t{Path(relative).name}\n"
            for relative in SYSTEMD_UNITS["staging"]
        )
    )
    (source / "deploy/migration-checkpoints.tsv").write_text(
        f"{MIGRATION_EXPECTED}\t{MIGRATION_TARGET}\n"
        f"{MIGRATION_TARGET}\t{MIGRATION_TARGET}\n"
    )
    postgres_policy = source / "deploy/postgres"
    postgres_policy.mkdir(parents=True)
    for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql"):
        shutil.copy2(ROOT / "deploy/postgres" / name, postgres_policy / name)
    (source / ".gitignore").write_text(
        "venv/\n.runtime-release.env\n.runtime-deploy/\n"
    )
    (source / "requirements.txt").write_text("deployment-test==1\n")
    (source / "constraints.txt").write_text("deployment-test==1\n")
    (source / "alembic.ini").write_text("[alembic]\n")
    (source / "release.txt").write_text("old release\n")
    _git(source, "add", ".")
    _git(source, "commit", "-qm", "old release")
    _git(source, "remote", "add", "origin", str(origin))
    _git(source, "push", "-q", "-u", "origin", "staging")
    _git(tmp_path, "clone", "-q", "--branch", "staging", str(origin), str(checkout))
    old_sha = _git(checkout, "rev-parse", "HEAD").stdout.strip()

    fake_python = """#!/usr/bin/env bash
set -euo pipefail
printf 'python:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "${DEPLOY_FAIL_PHASE:-}" == "preflight" && "${1:-}" == "scripts/preflight_runtime.py" ]]; then
    exit 41
fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "dependency" && "${1:-} ${2:-} ${3:-}" == "-m pip install" ]]; then
    exit 42
fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "health" && "${1:-}" == "scripts/validate_health_response.py" ]]; then
    exit 44
fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "alembic" && "${1:-}" == "scripts/apply_migration_checkpoint.py" ]]; then
    exit 43
fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "current-revision" && "${1:-}" == "scripts/verify_migration_revision.py" ]]; then
    exit 46
fi
if [[ "${DEPLOY_FAIL_PHASE:-}" == "acl" && "${1:-}" == *"/scripts/converge_runtime_acls.py" ]]; then
    exit 45
fi
if [[ "${DEPLOY_BLOCK_PHASE:-}" == "${1:-}" ]]; then
    sleep 30
fi
"""
    fake_alembic = """#!/usr/bin/env bash
set -euo pipefail
printf 'alembic:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
if [[ "${DEPLOY_FAIL_PHASE:-}" == "alembic" ]]; then
    exit 43
fi
"""
    _write_executable(checkout / "venv/bin/python", fake_python)
    _write_executable(checkout / "venv/bin/alembic", fake_alembic)
    fake_bin = tmp_path / "fake-bin"
    _write_executable(
        fake_bin / "sudo",
        """#!/usr/bin/env bash
set -euo pipefail
printf 'sudo:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
""",
    )
    _write_executable(
        fake_bin / "systemctl",
        """#!/usr/bin/env bash
set -euo pipefail
printf 'systemctl:%s\\n' "$*" >> "${DEPLOY_TEST_LOG:?}"
""",
    )
    _write_executable(
        fake_bin / "curl",
        """#!/usr/bin/env bash
set -euo pipefail
printf '{"status":"ok","db":"ok","environment":"wrong","release_sha":"wrong"}'
""",
    )
    return source, checkout, old_sha


def _deploy_environment(log_path: Path, *, fail_phase: str = "") -> dict[str, str]:
    return {
        **os.environ,
        "TARGET_BRANCH": "staging",
        "SERVICE_NAME": "verdaxis-backend-staging-test.service",
        "HEALTH_URL": "https://health.invalid/health/ready",
        "DEPLOY_ENVIRONMENT": "staging",
        "DEPLOY_TEST_LOG": str(log_path),
        "DEPLOY_FAIL_PHASE": fail_phase,
        "DEPLOY_STATE_DIR": str(log_path.parent / "runtime-deploy-state"),
        "HEALTH_ATTEMPTS": "1",
        "PATH": f"{log_path.parent / 'fake-bin'}{os.pathsep}{os.environ['PATH']}",
    }


def _approved_deploy_environment(
    log_path: Path,
    release_sha: str,
    *,
    fail_phase: str = "",
) -> dict[str, str]:
    environment = _deploy_environment(log_path, fail_phase=fail_phase)
    environment.update(
        {
            "APPROVED_RELEASE_SHA": release_sha,
            "MIGRATION_APPROVED_SOURCE_SHA": release_sha,
            "MIGRATION_EXPECTED_CURRENT_REVISION": MIGRATION_EXPECTED,
            "MIGRATION_TARGET_REVISION": MIGRATION_TARGET,
        }
    )
    return environment


def _push_new_release(source: Path) -> str:
    (source / "release.txt").write_text("new release\n")
    _git(source, "add", "release.txt")
    _git(source, "commit", "-qm", "new release")
    _git(source, "push", "-q", "origin", "staging")
    return _git(source, "rev-parse", "HEAD").stdout.strip()


def test_deploy_dry_run_only_attests_pinned_archive_without_candidate_execution(tmp_path):
    source, checkout, current_sha = _deployment_checkout(tmp_path)
    candidate_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh", "--dry-run"],
        cwd=checkout,
        env=_deploy_environment(log_path),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode == 0, result.stderr
    commands = log_path.read_text().splitlines() if log_path.exists() else []
    assert not any("python:" in command for command in commands)
    assert not any("alembic:" in command for command in commands)
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == current_sha
    assert current_sha != candidate_sha
    assert not (checkout / ".runtime-release.env").exists()
    assert not (checkout / ".runtime-deploy").exists()
    assert f"APPROVED_RELEASE_SHA={candidate_sha}" in result.stdout
    assert "Candidate Python code was not executed" in result.stdout
    for skipped in ("source update", "candidate application preflight", "dependency installation", "migration upgrade", "service restart", "health gate"):
        assert skipped in result.stdout


@pytest.mark.parametrize(
    "relative",
    [
        UNIT_MANIFEST,
        "deploy/migration-checkpoints.tsv",
        SYSTEMD_UNITS["staging"][0],
        "scripts/preflight_runtime.py",
    ],
)
def test_deploy_dry_run_rejects_candidate_symlink_artifacts(tmp_path, relative):
    source, checkout, current_sha = _deployment_checkout(tmp_path)
    secret = tmp_path / "operator-secret"
    secret.write_text("OPERATOR_SECRET=must-not-be-read\n")
    candidate = source / relative
    candidate.unlink()
    candidate.symlink_to(secret)
    _git(source, "add", "-A")
    _git(source, "commit", "-qm", "malicious candidate symlink")
    _git(source, "push", "-q", "origin", "staging")
    log_path = tmp_path / "commands.log"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh", "--dry-run"],
        cwd=checkout,
        env=_deploy_environment(log_path),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "regular committed file" in result.stderr
    assert "OPERATOR_SECRET" not in result.stdout + result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == current_sha
    commands = log_path.read_text().splitlines() if log_path.exists() else []
    assert not any("python:" in command for command in commands)


def test_real_deploy_refuses_missing_migration_checkpoint_before_source_mutation(
    tmp_path,
):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _deploy_environment(log_path)
    environment["APPROVED_RELEASE_SHA"] = new_sha

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "MIGRATION_APPROVED_SOURCE_SHA" in result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == old_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


def test_real_deploy_refuses_symbolic_checkpoint_before_source_mutation(tmp_path):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(log_path, new_sha)
    environment["MIGRATION_TARGET_REVISION"] = "HEAD"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "literal migration" in result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == old_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


def test_real_deploy_refuses_moved_remote_before_source_mutation(tmp_path):
    source, checkout, approved_sha = _deployment_checkout(tmp_path)
    _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(log_path, approved_sha)

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "moved after approval" in result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == approved_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


def test_real_deploy_refuses_unallowlisted_checkpoint_before_source_mutation(
    tmp_path,
):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(log_path, new_sha)
    environment["MIGRATION_TARGET_REVISION"] = "pref_20260709_user_preferences"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "not allowlisted" in result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == old_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


def test_real_deploy_refuses_unexpected_live_revision_before_source_mutation(
    tmp_path,
):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(
        log_path, new_sha, fail_phase="current-revision"
    )

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == old_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()
    commands = log_path.read_text().splitlines()
    revision_check = next(
        command
        for command in commands
        if "scripts/verify_migration_revision.py" in command
    )
    assert f"--expected {MIGRATION_EXPECTED}" in revision_check

    source_text = (checkout / "scripts/deploy.sh").read_text()
    main = source_text.index('CURRENT_BRANCH="$("${GIT[@]}" branch --show-current)"')
    assert source_text.index("acquire_deploy_lock", main) < source_text.index(
        "scripts/verify_migration_revision.py", main
    ) < source_text.index('write_deploy_state "blocked"', main)


def test_real_deploy_rejects_non_numeric_health_controls_without_execution(tmp_path):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    marker = tmp_path / "arithmetic-injection"
    environment = _approved_deploy_environment(log_path, new_sha)
    environment["HEALTH_ATTEMPTS"] = f"1+$(touch {marker})"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "HEALTH_ATTEMPTS" in result.stderr
    assert not marker.exists()
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == old_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


def test_deploy_dry_run_rejects_malformed_checkpoint_policy(tmp_path):
    source, checkout, current_sha = _deployment_checkout(tmp_path)
    (source / "deploy/migration-checkpoints.tsv").write_text("head\trh\textra\n")
    _git(source, "add", "deploy/migration-checkpoints.tsv")
    _git(source, "commit", "-qm", "malformed checkpoint policy")
    _git(source, "push", "-q", "origin", "staging")
    log_path = tmp_path / "commands.log"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh", "--dry-run"],
        cwd=checkout,
        env=_deploy_environment(log_path),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "Invalid migration checkpoint policy" in result.stderr
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == current_sha
    assert not (log_path.parent / "runtime-deploy-state/staging.state").exists()


@pytest.mark.parametrize(
    "fail_phase", ["preflight", "dependency", "alembic", "acl"]
)
def test_deploy_failure_keeps_selected_code_and_identity_aligned_and_guarded(
    tmp_path, fail_phase
):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    (checkout / ".runtime-release.env").write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={old_sha}\n"
    )
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(
        log_path, new_sha, fail_phase=fail_phase
    )

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert _git(checkout, "rev-parse", "HEAD").stdout.strip() == new_sha
    assert (checkout / ".runtime-release.env").read_text() == (
        f"ENVIRONMENT=staging\nRELEASE_SHA={new_sha}\n"
        f"MIGRATION_REVISION={MIGRATION_TARGET}\n"
    )
    assert (log_path.parent / "runtime-deploy-state/staging.state").is_file()
    assert "durable state stays fail-closed" in result.stderr


def test_failed_post_restart_health_stops_service_and_restores_guard(tmp_path):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    (checkout / ".runtime-release.env").write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={old_sha}\n"
    )
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(
        log_path, new_sha, fail_phase="health"
    )

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert (checkout / ".runtime-release.env").read_text() == (
        f"ENVIRONMENT=staging\nRELEASE_SHA={new_sha}\n"
        f"MIGRATION_REVISION={MIGRATION_TARGET}\n"
    )
    assert (log_path.parent / "runtime-deploy-state/staging.state").is_file()
    commands = log_path.read_text().splitlines()
    restart = "sudo:systemctl restart verdaxis-backend-staging-test.service"
    stop = "sudo:systemctl stop verdaxis-backend-staging-test.service"
    assert restart in commands
    assert stop in commands
    assert commands.index(restart) < commands.index(stop)
    checkpoint = next(
        command
        for command in commands
        if "scripts/apply_migration_checkpoint.py" in command
    )
    acl_convergence = next(
        command
        for command in commands
        if "/scripts/converge_runtime_acls.py" in command
    )
    assert f"--source-sha {new_sha}" in checkpoint
    assert f"--approved-source-sha {new_sha}" in checkpoint
    assert f"--expected-current {MIGRATION_EXPECTED}" in checkpoint
    assert f"--target {MIGRATION_TARGET}" in checkpoint
    assert " upgrade head" not in checkpoint
    assert commands.index(checkpoint) < commands.index(acl_convergence)
    assert commands.index(acl_convergence) < commands.index(restart)


def test_deploy_lock_serializes_concurrent_runs_and_state_survives_interruption(tmp_path):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    (checkout / ".runtime-release.env").write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={old_sha}\n"
    )
    new_sha = _push_new_release(source)
    log_path = tmp_path / "commands.log"
    environment = _approved_deploy_environment(log_path, new_sha)
    environment["DEPLOY_BLOCK_PHASE"] = "scripts/preflight_runtime.py"

    running = subprocess.Popen(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    state = log_path.parent / "runtime-deploy-state/staging.state"
    for _ in range(250):
        commands = log_path.read_text() if log_path.exists() else ""
        if state.exists() and "scripts/preflight_runtime.py" in commands:
            break
        import time

        time.sleep(0.02)
    assert state.exists()
    assert "scripts/preflight_runtime.py" in commands

    try:
        concurrent = subprocess.run(
            ["bash", "scripts/deploy.sh"],
            cwd=checkout,
            env={**environment, "DEPLOY_BLOCK_PHASE": ""},
            capture_output=True,
            text=True,
            check=False,
        )
        assert concurrent.returncode != 0
        assert "already running" in concurrent.stderr
    finally:
        os.killpg(running.pid, signal.SIGTERM)
        running.wait(timeout=5)
    assert state.exists()
    assert "DEPLOYMENT_STATE=blocked" in state.read_text()


def test_deploy_publishes_identity_before_selected_tree_execution():
    source = (ROOT / "scripts/deploy.sh").read_text()

    identity = source.index('write_release_artifact "$CURRENT_SHA"')
    assert identity < source.index("scripts/preflight_runtime.py", identity)
    assert identity < source.index("pip install", identity)
    assert identity < source.index("scripts/apply_migration_checkpoint.py", identity)
    assert "alembic upgrade head" not in source
    assert "reset --hard" not in source
    assert '"${GIT[@]}" checkout --' not in source


def test_real_deploy_refuses_unconstrained_dependency_install(tmp_path):
    source, checkout, old_sha = _deployment_checkout(tmp_path)
    (checkout / ".runtime-release.env").write_text(
        f"ENVIRONMENT=staging\nRELEASE_SHA={old_sha}\n"
    )
    (source / "constraints.txt").unlink()
    _git(source, "add", "-A")
    _git(source, "commit", "-qm", "remove dependency constraints")
    _git(source, "push", "-q", "origin", "staging")
    new_sha = _git(source, "rev-parse", "HEAD").stdout.strip()
    log_path = tmp_path / "commands.log"

    result = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=checkout,
        env=_approved_deploy_environment(log_path, new_sha),
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode != 0
    assert "constraints.txt is required" in result.stderr
    assert "pip install" not in log_path.read_text()


@pytest.mark.parametrize(
    "unit_name",
    [
        "verdaxis-backend.service",
        "verdaxis-backend-staging.service",
        "verdaxis-news-refresh.service",
        "verdaxis-news-refresh-staging.service",
        "verdaxis-product-analytics-prune.service",
        "verdaxis-product-analytics-prune-staging.service",
    ],
)
def test_every_runtime_service_fails_closed_during_deployment(unit_name):
    content = (ROOT / "deploy/systemd" / unit_name).read_text()
    backend_dir = (
        "/home/verdaxis-prod/verdaxis/staging/be"
        if "staging" in unit_name
        else "/home/verdaxis-prod/verdaxis/prod/be"
    )

    state = f"{backend_dir}/.runtime-deploy/{'staging' if 'staging' in unit_name else 'production'}.state"
    assert state in content
    assert "DEPLOYMENT_STATE=restart-authorized" in content


def test_systemd_installer_is_idempotent_preflights_and_never_starts_services():
    source = (ROOT / "scripts/install_systemd_units.sh").read_text()

    assert "systemd-analyze verify" in source
    assert "preflight_runtime.py" not in source
    assert "alembic current --check-heads" not in source
    assert "cmp --silent" in source or "cmp -s" in source
    assert "systemctl daemon-reload" in source
    assert "PENDING_STATE" in source
    assert source.index("systemctl daemon-reload") < source.index(
        'rm -f -- "$PENDING_STATE"'
    )
    assert "systemctl start" not in source
    assert "systemctl restart" not in source
    assert "systemctl enable" not in source


def _load_systemd_source_verifier():
    path = ROOT / "scripts" / "verify_systemd_source.py"
    assert path.exists(), "release-bound systemd source verifier is required"
    spec = importlib.util.spec_from_file_location("verify_systemd_source", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _committed_unit_source(
    tmp_path: Path, environment: str = "production"
) -> tuple[Path, str, tuple[str, ...]]:
    root = tmp_path / "approved-release"
    unit_dir = root / "deploy" / "systemd"
    unit_dir.mkdir(parents=True)
    units = SYSTEMD_UNITS[environment]
    for relative in units:
        (root / relative).write_text(f"[Unit]\nDescription={Path(relative).name}\n")
    (root / UNIT_MANIFEST).write_text(
        "".join(f"{environment}\t{Path(relative).name}\n" for relative in units)
    )
    preflight = root / "scripts" / "preflight_runtime.py"
    preflight.parent.mkdir()
    preflight.write_text("# approved preflight\n")
    subprocess.run(["git", "init", "-q"], cwd=root, check=True)
    subprocess.run(
        ["git", "config", "user.email", "runtime-test@example.invalid"],
        cwd=root,
        check=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "Runtime Test"], cwd=root, check=True
    )
    subprocess.run(["git", "add", "."], cwd=root, check=True)
    subprocess.run(["git", "commit", "-qm", "approved units"], cwd=root, check=True)
    source_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    return root, source_ref, units


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_wrong_sha_in_every_mode(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, _source_ref, units = _committed_unit_source(tmp_path)

    with pytest.raises(verifier.SourceProvenanceError, match="source ref"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref="b" * 40,
            environment="production",
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_dirty_unit_bytes(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    (root / units[0]).write_text("[Unit]\nDescription=unapproved bytes\n")

    with pytest.raises(verifier.SourceProvenanceError, match="dirty"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            environment="production",
        )


@pytest.mark.parametrize("relative_kind", ["manifest", "unit"])
def test_systemd_source_provenance_refuses_symlink_artifacts(
    tmp_path, relative_kind
):
    verifier = _load_systemd_source_verifier()
    root, _source_ref, units = _committed_unit_source(tmp_path)
    relative = UNIT_MANIFEST if relative_kind == "manifest" else units[0]
    secret = tmp_path / "operator-secret"
    secret.write_text("OPERATOR_SECRET=must-not-be-read\n")
    candidate = root / relative
    candidate.unlink()
    candidate.symlink_to(secret)
    subprocess.run(["git", "add", "-A"], cwd=root, check=True)
    subprocess.run(
        ["git", "commit", "-qm", "malicious candidate symlink"],
        cwd=root,
        check=True,
    )
    source_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()

    with pytest.raises(verifier.SourceProvenanceError, match=r"regular .*file"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            environment="production",
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_digest_mismatch_hidden_from_status(
    tmp_path, monkeypatch, mode
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    real_git = verifier._git

    def status_blind_git(source_root, *args, **kwargs):
        if args[:2] == ("status", "--porcelain"):
            return ""
        return real_git(source_root, *args, **kwargs)

    monkeypatch.setattr(verifier, "_git", status_blind_git)
    (root / units[0]).write_text("[Unit]\nDescription=hidden unapproved bytes\n")

    with pytest.raises(verifier.SourceProvenanceError, match="digest"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            environment="production",
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_hidden_non_unit_source_changes(
    tmp_path, mode
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    preflight = "scripts/preflight_runtime.py"
    subprocess.run(
        ["git", "update-index", "--assume-unchanged", preflight],
        cwd=root,
        check=True,
    )
    (root / preflight).write_text("# bypassed preflight\n")

    with pytest.raises(verifier.SourceProvenanceError, match="index flags"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            environment="production",
        )


@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_refuses_git_replacement_refs(tmp_path, mode):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    subprocess.run(
        ["git", "commit", "--allow-empty", "-qm", "replacement commit"],
        cwd=root,
        check=True,
    )
    replacement_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()
    subprocess.run(
        ["git", "checkout", "-q", "--detach", source_ref], cwd=root, check=True
    )
    subprocess.run(
        ["git", "replace", source_ref, replacement_ref], cwd=root, check=True
    )

    with pytest.raises(verifier.SourceProvenanceError, match="replacement refs"):
        verifier.verify_source_provenance(
            source_root=root,
            source_ref=source_ref,
            environment="production",
        )


@pytest.mark.parametrize("environment", sorted(SYSTEMD_UNITS))
@pytest.mark.parametrize("mode", ["dry-run", "apply"])
def test_systemd_source_provenance_accepts_exact_clean_release(
    tmp_path, environment, mode
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path, environment)

    digests = verifier.verify_source_provenance(
        source_root=root,
        source_ref=source_ref,
        environment=environment,
    )

    assert set(digests) == set(units)
    assert all(len(digest) == 64 for digest in digests.values())


@pytest.mark.parametrize("environment", sorted(SYSTEMD_UNITS))
def test_systemd_archive_contains_exact_environment_unit_set_and_digests(
    tmp_path, environment
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path, environment)

    archive = verifier.build_verified_unit_archive(
        source_root=root,
        source_ref=source_ref,
        environment=environment,
    )

    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
        assert set(bundle.getnames()) == {*units, UNIT_MANIFEST, "SHA256SUMS"}
        manifest = bundle.extractfile("SHA256SUMS")
        assert manifest is not None
        digest_lines = manifest.read().decode().splitlines()

        expected_lines = []
        for relative in (UNIT_MANIFEST, *units):
            committed = subprocess.check_output(
                ["git", "show", f"{source_ref}:{relative}"], cwd=root
            )
            expected_lines.append(f"{hashlib.sha256(committed).hexdigest()}  {relative}")
            archived = bundle.extractfile(relative)
            assert archived is not None
            assert archived.read() == committed
        assert digest_lines == expected_lines


def test_systemd_manifest_extends_exact_archive_without_installer_code_changes(
    tmp_path,
):
    verifier = _load_systemd_source_verifier()
    root, _source_ref, units = _committed_unit_source(tmp_path)
    additions = (
        "deploy/systemd/verdaxis-auth-maintenance.service",
        "deploy/systemd/verdaxis-auth-maintenance.timer",
    )
    for relative in additions:
        (root / relative).write_text(
            f"[Unit]\nDescription={Path(relative).name}\n"
        )
    with (root / UNIT_MANIFEST).open("a") as manifest:
        for relative in additions:
            manifest.write(f"production\t{Path(relative).name}\n")
    subprocess.run(["git", "add", "."], cwd=root, check=True)
    subprocess.run(
        ["git", "commit", "-qm", "extend audited unit manifest"],
        cwd=root,
        check=True,
    )
    source_ref = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=root, text=True
    ).strip()

    archive = verifier.build_verified_unit_archive(
        source_root=root,
        source_ref=source_ref,
        environment="production",
    )

    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
        assert set(bundle.getnames()) == {
            *units,
            *additions,
            UNIT_MANIFEST,
            "SHA256SUMS",
        }
    installer = (ROOT / "scripts/install_systemd_units.sh").read_text()
    assert "verdaxis-auth-maintenance" not in installer
    # The security checkpoint landed the audited auth-maintenance units on
    # this tree: bytes exist and both environments are manifest-declared,
    # still without naming them in installer code.
    committed_manifest = (ROOT / UNIT_MANIFEST).read_text()
    for relative in additions:
        assert (ROOT / relative).exists()
        assert f"production\t{Path(relative).name}" in committed_manifest
        staging_name = Path(relative).name.replace(
            "verdaxis-auth-maintenance", "verdaxis-auth-maintenance-staging"
        )
        assert (ROOT / "deploy/systemd" / staging_name).exists()
        assert f"staging\t{staging_name}" in committed_manifest


def test_systemd_manifest_rejects_cross_environment_filename_collision():
    verifier = _load_systemd_source_verifier()
    manifest = (
        b"production\tverdaxis-backend.service\n"
        b"staging\tverdaxis-backend.service\n"
    )

    with pytest.raises(verifier.SourceProvenanceError, match="duplicate"):
        verifier._parse_unit_manifest(manifest, "staging")


@pytest.mark.parametrize(
    "manifest",
    [
        b"production\tverdaxis-backend-staging.service\n",
        b"staging\tverdaxis-backend.service\n",
    ],
)
def test_systemd_manifest_rejects_cross_environment_unit_names(manifest):
    verifier = _load_systemd_source_verifier()

    with pytest.raises(verifier.SourceProvenanceError, match="environment"):
        verifier._parse_unit_manifest(manifest, "staging")


def test_systemd_archive_ignores_concurrent_worktree_mutation_after_attestation(
    tmp_path, monkeypatch
):
    verifier = _load_systemd_source_verifier()
    root, source_ref, units = _committed_unit_source(tmp_path)
    target = root / units[0]
    approved_bytes = target.read_bytes()
    real_read = verifier._read_working_unit

    def mutate_after_read(path):
        candidate = real_read(path)
        if path == target:
            path.write_text("[Unit]\nDescription=concurrent unapproved mutation\n")
        return candidate

    monkeypatch.setattr(verifier, "_read_working_unit", mutate_after_read)
    archive = verifier.build_verified_unit_archive(
        source_root=root,
        source_ref=source_ref,
        environment="production",
    )

    assert target.read_bytes() != approved_bytes
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as bundle:
        archived = bundle.extractfile(units[0])
        assert archived is not None
        assert archived.read() == approved_bytes


def test_systemd_installer_requires_independent_environment_and_source_ref():
    source = (ROOT / "scripts" / "install_systemd_units.sh").read_text()

    assert "--environment" in source
    assert "--source-ref" in source
    assert "archive --format=tar" in source
    assert "/home/verdaxis-prod/verdaxis/prod/be" in source
    assert "/home/verdaxis-prod/verdaxis/staging/be" in source
    assert UNIT_MANIFEST in source
    for units in SYSTEMD_UNITS.values():
        for relative in units:
            assert Path(relative).name not in source
    assert "SHA256SUMS" in source
    assert "sudo mktemp -d" in source
    assert "sha256sum --check" in source
    assert "verify_staged_blob" in source
    assert 'sudo test -L "$staged"' in source
    assert 'show "$SOURCE_REF:$relative"' in source
    assert 'UNIT_SOURCES+=("$STAGING_DIR/deploy/systemd/$unit_name")' in source
    assert 'UNIT_SOURCES+=("$SOURCE_ROOT/deploy/systemd/$unit_name")' not in source
    assert "preflight_backend production" not in source
    assert "preflight_backend staging" not in source
    assert "venv/bin" not in source
    assert "preflight_runtime.py" not in source
    assert "alembic current" not in source
    assert source.index("archive --format=tar") < source.index(
        'if [[ "$MODE" == "dry-run" ]]'
    )
    assert "DESTINATION_TEMPS" in source
    assert "rollback_replacements" in source
    assert 'sudo test -f "$destination"' in source
    assert 'sudo test ! -L "$destination"' in source
    assert "/usr/bin/git" in source
    assert "/usr/bin/flock" in source
    lock_gate = '"$EUID" -ne 0 || "${VERDAXIS_SYSTEMD_INSTALL_LOCKED:-}" != "1"'
    assert lock_gate in source
    assert 'PENDING_DIR="/var/lib/verdaxis/systemd-units"' in source
    assert "INSTALL_PENDING_DIR" not in source


def test_live_deploy_uses_canonical_guard_state_and_trusted_tool_path():
    source = (ROOT / "scripts/deploy.sh").read_text()

    assert 'DEPLOY_STATE_DIR="$BACKEND_DIR/.runtime-deploy"' in source
    canonical = source.index('if [[ "$CANONICAL_DEPLOY_LAYOUT" == "1" ]]')
    canonical_state = source.index(
        'DEPLOY_STATE_DIR="$BACKEND_DIR/.runtime-deploy"', canonical
    )
    custom_override = source.index('DEPLOY_STATE_DIR="${DEPLOY_STATE_DIR:-')
    assert canonical < canonical_state < custom_override
    assert 'PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"' in source
    assert "GIT=(/usr/bin/git" in source
    assert 'TARGET_BRANCH="$DEFAULT_BRANCH"' in source
    assert "unset DATABASE_URL MIGRATOR_DATABASE_URL" in source
    assert "unset PYTHONHOME PYTHONPATH" in source
    assert 'export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1' in source
    assert "PIP_CONFIG_FILE=/dev/null" in source
    assert "./venv/bin/python -m pip install -r requirements.txt -c constraints.txt" in source
    assert "./venv/bin/python -m pip check" in source
    assert source.index("umask 022", source.index("if [[ -f requirements.txt ]]")) < source.index(
        "pip install", source.index("if [[ -f requirements.txt ]]")
    )
    assert "chmod -R a+rX venv" in source
    assert 'SCRIPT_DIR="$(cd -- "${BASH_SOURCE[0]%/*}" && pwd -P)"' in source
    assert 'dirname "${BASH_SOURCE[0]}"' not in source
    assert "HEALTH_ATTEMPTS must be an integer from 1 to 120" in source

    installer = (ROOT / "scripts/install_systemd_units.sh").read_text()
    assert 'export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1' in installer
    assert installer.index("VERDAXIS_SYSTEMD_INSTALL_LOCKED") < installer.index(
        'STAGING_DIR="$(sudo mktemp'
    )
