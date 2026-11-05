"""Portable subprocess contracts for the shell-owned job handover."""
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HELPER = ROOT / "scripts/rust_w2_handover.sh"
FIXTURE = ROOT / "deploy/tests/w2-handover-contract.sh"


def test_shell_handover_contract():
    result = subprocess.run(["bash", str(FIXTURE)], cwd=ROOT, text=True, capture_output=True)
    assert result.returncode == 0, result.stdout + result.stderr


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


def test_usage_refuses_unbound_apply():
    result = subprocess.run(["bash", str(HELPER), "--action", "advance"], text=True, capture_output=True)
    assert result.returncode == 2, result.stdout + result.stderr
    assert "usage:" in result.stderr
