"""Portable subprocess contracts for the shell-owned job handover."""
import json
import os
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "scripts/rust_w2_handover.sh"
FIXTURE = ROOT / "deploy/tests/w2-handover-contract.sh"


def test_shell_handover_contract():
    result = subprocess.run(["bash", str(FIXTURE)], cwd=ROOT, text=True, capture_output=True)
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("environment", ["production", "staging"])
@pytest.mark.parametrize("failed_surface", ["", "local", "public"])
def test_ready_checks_rust_policy_and_public_identity(tmp_path, environment, failed_surface):
    policy = json.loads((ROOT / f"deploy/rust/{environment}-policy.json").read_text())
    public_host = "api.verdaxis.exchange" if environment == "production" else "api-staging.verdaxis.exchange"
    public_url = f"https://{public_host}/health/ready"
    calls = tmp_path / "calls"
    ready = dict(status="ok", db="ok", environment=environment, release_sha="a" * 40)
    shell = r'''
source "$HELPER"
function /usr/bin/curl() {
    local url=${@: -1}
    printf '%s\n' "$url" >> "$CALLS"
    if [[ $FAILED_SURFACE == local && $url == http://* || $FAILED_SURFACE == public && $url == https://* ]]; then
        printf '%s\n' "$BAD_READY"
    else
        printf '%s\n' "$READY"
    fi
}
verify_ready
'''
    env = dict(os.environ, HELPER=str(HELPER), SOURCE_ROOT=str(ROOT), ENVIRONMENT=environment,
               SOURCE_REF="a" * 40, CALLS=str(calls), READY=json.dumps(ready),
               BAD_READY=json.dumps(dict(ready, release_sha="b" * 40)), FAILED_SURFACE=failed_surface)
    result = subprocess.run(["bash", "-c", shell], cwd=ROOT, env=env, text=True, capture_output=True)
    if failed_surface:
        assert result.returncode != 0, "readiness accepted a different release"
    else:
        assert result.returncode == 0, result.stdout + result.stderr
        assert calls.read_text().splitlines() == [policy["health_url"], public_url]


@pytest.mark.parametrize("payload", ["", "{}", "[]", '{"status":"ok"}', "not-json"])
def test_ready_refuses_incomplete_local_document(payload):
    env = dict(os.environ, HELPER=str(HELPER), SOURCE_ROOT=str(ROOT), ENVIRONMENT="staging",
               SOURCE_REF="a" * 40, PAYLOAD=payload)
    result = subprocess.run(["bash", "-c", r'''
source "$HELPER"
function /usr/bin/curl() { printf '%s' "$PAYLOAD"; }
verify_ready
'''], cwd=ROOT, env=env, text=True, capture_output=True)
    assert result.returncode != 0, "incomplete readiness document was accepted"


def test_no_deployed_python_dependency():
    assert HELPER.exists(), "shell handover entry point is absent"
    text = HELPER.read_text()
    assert "eval " not in text
    assert "bash -c" not in text
    assert "python3" not in text
    assert "sudo -n" in text  # operator entry is noninteractive


def test_installer_is_noninteractive_and_environment_scoped():
    installer = (ROOT / "scripts/install_systemd_units.sh").read_text()
    assert "exec sudo -n /usr/bin/env -i" in installer
    assert 'HOST_INSTALL_ARGS=(--environment "$DEPLOY_ENVIRONMENT"' in installer
    host = (ROOT / "deploy/scripts/install-host-automation.sh").read_text()
    assert "--environment)" in host
    assert "artifact_in_environment" in host
    assert "--scope)" in host
    assert 'HOST_SCOPE="environment"' in host
    assert 'if [[ $HOST_SCOPE == environment ]]' in host


def test_handover_serializes_with_deploy_and_installer():
    text = HELPER.read_text()
    assert '"$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.lock"' in text
    assert '"$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.restart.lock"' in text
    assert '"/run/lock/verdaxis-systemd-units-${ENVIRONMENT}.lock"' in text
    assert "verify_fence_inventory" in text
    assert "VERDAXIS_SYSTEMD_INSTALL_LOCK_FD" in text
    installer = (ROOT / "scripts/install_systemd_units.sh").read_text()
    assert "VERDAXIS_SYSTEMD_INSTALL_LOCK_FD" in installer
    assert "/proc/self/fd" in installer


def test_stale_deploy_state_is_refused_before_restart_lock():
    text = HELPER.read_text()
    main_lock = text.index('/usr/bin/flock -n 8')
    restart_lock = text.index('exec 7>>', main_lock)
    guarded_window = text[main_lock:restart_lock]
    state_path = '$SOURCE_ROOT/.runtime-deploy/$ENVIRONMENT.state'
    assert f'! -e {state_path}' in guarded_window, "present state must refuse under the main lock"
    assert f'! -L {state_path}' in guarded_window, "dangling state must refuse before restart lock"
    assert "return 1" in guarded_window, "state refusal must stop before restart authorization can revive"


def test_status_separates_active_jobs_from_preserved_and_retired_timers():
    text = HELPER.read_text()
    assert "expected=6; [[ $ENVIRONMENT == production ]] || expected=4" in text
    assert "preserved_timer=verdaxis-local-db-backup.timer" in text
    assert "inactive_retired_timer=verdaxis-order-expiry-reminders-staging.timer" in text
    assert "timer_inventory_scope_unresolved" not in text
    assert "window=NOT_ACCEPTED" in text


def test_usage_refuses_unbound_apply():
    result = subprocess.run(["bash", str(HELPER), "--action", "advance"], text=True, capture_output=True)
    assert result.returncode == 2, result.stdout + result.stderr
    assert "usage:" in result.stderr
