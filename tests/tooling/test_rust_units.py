"""The Rust and maintenance units, their manifest entries, and the bootstrap.

The checks read the committed unit, policy and sudoers files, or run the real
deploy policy function, systemd-analyze and visudo. No test starts a service,
installs a unit or touches a host.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
UNIT = "deploy/systemd/verdaxis-rust-staging.service"
MANIFEST = "deploy/systemd/runtime-units.manifest"
POLICY = "deploy/rust/staging-policy.json"
DEPLOY = "scripts/deploy.sh"
INSTALLER = "scripts/install_systemd_units.sh"
STAGING_BACKEND = "/home/verdaxis-prod/verdaxis/staging/be"
RUST_UNIT_NAME = "verdaxis-rust-staging.service"
BIND = "127.0.0.1:8011"
RELEASE_SHA = "a" * 40


def _text(relative: str) -> str:
    return (ROOT / relative).read_text()


@pytest.mark.parametrize(
    "bootstrap_step, command",
    [
        # f4663bf4091ea4261875414f017cd27cd5a2ceb1 deploy.sh:560.
        ("old-helper-on-bootstrap", 'systemd-analyze verify "${CANDIDATE_UNITS[@]}"'),
        ("bootstrap-installer", None),
        ("bootstrap-helper-on-activation", None),
    ],
)
def test_bootstrap_verifies_without_a_rust_artifact(tmp_path, bootstrap_step, command):
    """Verify real unit bytes, not a fake binary that hides first-install failures."""
    analyzer = shutil.which("systemd-analyze")
    if analyzer is None:
        pytest.skip("systemd-analyze is required for the Linux bootstrap gate")
    if command is None:
        source = _text(INSTALLER if bootstrap_step == "bootstrap-installer" else DEPLOY)
        command = next(
            line.strip() for line in source.splitlines()
            if "systemd-analyze verify" in line
        ).removeprefix("sudo -n ").removeprefix("sudo ")

    root = tmp_path / "root"
    units = root / "etc/systemd/system"
    units.mkdir(parents=True)
    unit = units / RUST_UNIT_NAME
    # systemd 249 does not support verify --root. Relocate only the checkout
    # prefix into this test user's private tree; keep every directive and flag.
    backend = root / "be"
    python = backend / "venv/bin/python"
    python.parent.mkdir(parents=True)
    python.symlink_to(shutil.which("python3"))
    unit.write_text(_text(UNIT).replace(STAGING_BACKEND, str(backend)))
    (units / "postgresql.service").write_text(
        "[Service]\nType=oneshot\nExecStart=/bin/true\n"
    )
    for name in ("sysinit.target", "basic.target", "network-online.target",
                 "shutdown.target"):
        (units / name).write_text("[Unit]\nDescription=fixture target\n")
    assert not (backend / ".runtime-rust").exists()
    # Preserve each caller's array invocation. No service starts, host files or
    # privileges are involved; SYSTEMD_UNIT_PATH supplies only dependency stubs.
    script = (
        'CANDIDATE_UNITS=("$UNIT_PATH")\nUNIT_SOURCES=("$UNIT_PATH")\n'
        + command
    )
    result = subprocess.run(
        ["bash", "-c", script], capture_output=True, text=True, check=False,
        env={**os.environ, "SYSTEMD_UNIT_PATH": str(units), "UNIT_PATH": str(unit)},
    )
    print(f"{bootstrap_step}: VERIFY_EXIT={result.returncode}\n{result.stderr}")
    assert result.returncode == 0, result.stderr


def test_rust_unit_bind_and_service_name_match_the_committed_policy():
    policy = json.loads(_text(POLICY))
    unit = _text(UNIT)

    assert Path(UNIT).name == policy["service_name"] == RUST_UNIT_NAME
    assert policy["bind"] == BIND
    assert policy["health_url"] == f"http://{BIND}/health/ready"
    assert f"--bind {policy['bind']}" in unit


def _assert_runtime_directives(unit: str, suffix: str) -> None:
    # A main-process watchdog in both environments, and exactly one credential:
    # the environment's own KYC document key ring.
    assert "WatchdogSec=60s\n" in unit
    assert "NotifyAccess=main\n" in unit
    credentials = [line for line in unit.splitlines() if line.startswith(
        ("LoadCredential=", "SetCredential=", "ImportCredential=", "LoadCredentialEncrypted=", "SetCredentialEncrypted=")
    )]
    assert credentials == [
        f"LoadCredential=kyc-document-keys:/etc/verdaxis/kyc-document-keys{suffix}.json",
    ]
    assert "# Key source must be regular root:root 0400; use an empty placeholder without a key ring." in unit


@pytest.mark.parametrize("suffix", ["", "-staging"])
def test_rust_unit_has_a_watchdog_and_only_its_own_key_credential(suffix):
    unit = _text(f"deploy/systemd/verdaxis-rust{suffix}.service")
    _assert_runtime_directives(unit, suffix)
    other = "-staging" if suffix == "" else ""
    for invalid in (
        unit + "\nSetCredential=another-key:\n",
        unit + "\nImportCredential=*\n",
        unit.replace(f"kyc-document-keys{suffix}.json", f"kyc-document-keys{other}.json"),
        unit.replace("WatchdogSec=60s", "WatchdogSec=0"),
        unit.replace("NotifyAccess=main", "NotifyAccess=all"),
    ):
        with pytest.raises(AssertionError):
            _assert_runtime_directives(invalid, suffix)


@pytest.mark.parametrize(
    ("unit_name", "backend_dir", "environment"),
    [
        ("verdaxis-rust.service", "/home/verdaxis-prod/verdaxis/prod/be", "production"),
        ("verdaxis-rust-staging.service", STAGING_BACKEND, "staging"),
    ],
)
def test_rust_units_use_the_lock_aware_restart_guard(unit_name, backend_dir, environment):
    content = _text(f"deploy/systemd/{unit_name}")
    command = (
        "ExecStartPre=/usr/bin/env -i PATH=/usr/bin:/bin /bin/bash "
        f"{backend_dir}/deploy/scripts/verify-deploy-start-guard.sh --environment {environment} "
        "--expected-release-sha ${RELEASE_SHA}"
    )
    assert command in content
    assert "DEPLOYMENT_STATE=restart-authorized" not in content


@pytest.mark.parametrize(
    ("unit_name", "backend_dir", "environment"),
    [
        ("verdaxis-news-refresh.service", "/home/verdaxis-prod/verdaxis/prod/be", "production"),
        ("verdaxis-news-refresh-staging.service", STAGING_BACKEND, "staging"),
        ("verdaxis-product-analytics-prune.service", "/home/verdaxis-prod/verdaxis/prod/be", "production"),
        ("verdaxis-product-analytics-prune-staging.service", STAGING_BACKEND, "staging"),
    ],
)
def test_maintenance_units_reject_every_present_deploy_state(unit_name, backend_dir, environment):
    content = _text(f"deploy/systemd/{unit_name}")
    state = f"{backend_dir}/.runtime-deploy/{environment}.state"
    assert f"ConditionPathExists=!{state}" in content
    assert f"ConditionPathIsSymbolicLink=!{state}" in content


def test_rust_unit_manifest_entries_are_environment_specific():
    entries = [
        tuple(line.split("\t"))
        for line in _text(MANIFEST).splitlines()
        if line.strip() and not line.startswith("#")
    ]
    assert ("staging", RUST_UNIT_NAME) in entries
    assert [entry for entry in entries if "rust" in entry[1]] == [
        ("production", "verdaxis-rust.service"),
        ("staging", RUST_UNIT_NAME),
    ]
    # The installer's manifest rules: unique filenames, staging suffix.
    assert len({name for _environment, name in entries}) == len(entries)
    assert RUST_UNIT_NAME.endswith("-staging.service")
    assert (ROOT / UNIT).is_file()


@pytest.mark.parametrize(
    "environment, unit_name, enabled",
    [("staging", RUST_UNIT_NAME, "1"), ("production", "verdaxis-rust.service", "1")],
)
def test_committed_policies_enable_the_loopback_service_in_both_environments(environment, unit_name, enabled):
    """Policy approval starts the loopback service; it does not publish a public route."""
    deploy = _text(DEPLOY)
    functions = deploy[
        deploy.index("load_rust_staging_policy() {"):
        deploy.index("resolve_provenance_public_key() {")
    ]
    policy = json.loads(_text(POLICY))
    assert policy["rust_enabled"] is True
    assert policy["environment"] == "staging"
    assert policy["bind"] == "127.0.0.1:8011"
    assert policy["health_url"] == "http://127.0.0.1:8011/health/ready"
    assert json.loads(_text("deploy/rust/production-policy.json"))["rust_enabled"] is True
    script = (
        "set -euo pipefail\n"
        f'DEPLOY_ENVIRONMENT="{environment}"\n'
        f'RUST_STAGING_POLICY_PATH="{POLICY}"\n'
        f'RUST_POLICY_PATH="deploy/rust/{environment}-policy.json"\n'
        f'UNIT_MANIFEST_PATH="{MANIFEST}"\n'
        f'RUST_UNIT_NAME="{unit_name}"\n'
        + functions
        + f'\nload_rust_staging_policy "{RELEASE_SHA}" "{ROOT}"\n'
        'printf "RUST_ENABLED=%s\\n" "$RUST_ENABLED"\n'
    )
    result = subprocess.run(
        ["bash", "-c", script, "policy-check"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert f"RUST_ENABLED={enabled}" in result.stdout


def _assert_staging_credential_boundary(unit: str) -> None:
    assert "/home/verdaxis-prod/verdaxis/prod/be" not in unit
    assert "MIGRATOR_DATABASE_URL" not in unit
    assert "/etc/verdaxis/kyc-document-keys.json" not in unit
    _assert_runtime_directives(unit, "-staging")


def test_rust_unit_names_no_production_path_or_migrator_credential():
    unit = _text(UNIT)
    _assert_staging_credential_boundary(unit)
    assert Path(UNIT).name.endswith("-staging.service")
    for forbidden in (
        unit.replace("kyc-document-keys-staging.json", "kyc-document-keys.json"),
        unit + "\nEnvironment=MIGRATOR_DATABASE_URL=unavailable\n",
        unit.replace(STAGING_BACKEND, "/home/verdaxis-prod/verdaxis/prod/be"),
    ):
        with pytest.raises(AssertionError):
            _assert_staging_credential_boundary(forbidden)


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_reviewed_sudoers_has_exact_environment_authority(environment):
    visudo = shutil.which("visudo") or "/usr/sbin/visudo"
    source = ROOT / "deploy/sudoers" / f"verdaxis-deploy-{environment}"
    result = subprocess.run([visudo, "-cf", str(source)], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    text = source.read_text()
    assert "*" not in text
    suffix = "-staging" if environment == "staging" else ""
    commands = {f"/usr/bin/systemctl {action} verdaxis-backend{suffix}.service" for action in ("restart", "stop")}
    commands |= {f"/usr/bin/systemctl {action} verdaxis-rust{suffix}.service" for action in ("restart", "stop", "disable", "enable")}
    actual = text.split("NOPASSWD: ", 1)[1].strip().split(", ")
    assert set(actual) == commands
    assert actual.count(f"/usr/bin/systemctl enable verdaxis-rust{suffix}.service") == 1
