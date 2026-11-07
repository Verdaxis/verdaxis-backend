"""Task 12c: the staging Rust unit, its manifest entry, and the bootstrap.

Every path the unit names must be a file the canonical deploy transaction
writes, and the unit's runtime account must be the account that wrote those
files. The checks here are static (unit text, deploy text, real helper runs) or
use the Task 12b fixture subprocess. No test starts a service, installs a unit,
or touches a host; the live proofs (systemd 249/255 syntax verification and an
artifact start on the staging ABI) are recorded in
``docs/rust-phase1/progress-task12c.md``.
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
UNIT = "deploy/systemd/verdaxis-rust-staging.service"
BACKEND_STAGING_UNIT = "deploy/systemd/verdaxis-backend-staging.service"
MANIFEST = "deploy/systemd/runtime-units.manifest"
POLICY = "deploy/rust/staging-policy.json"
DEPLOY = "scripts/deploy.sh"
PREPARE_RUNTIME = "scripts/prepare_rust_runtime.py"
PREPARE_BUDGET = "scripts/prepare_rust_budget.py"
INSTALLER = "scripts/install_systemd_units.sh"
STAGING_BACKEND = "/home/verdaxis-prod/verdaxis/staging/be"
STAGING_USER = "verdaxis-prod"
RUNTIME_DIR = f"{STAGING_BACKEND}/.runtime-rust"
RUST_UNIT_NAME = "verdaxis-rust-staging.service"
BIND = "127.0.0.1:8011"
RELEASE_SHA = "a" * 40
MIGRATION_REVISION = "kycg_20261014_staging"


def _text(relative: str) -> str:
    return (ROOT / relative).read_text()


def _module(relative: str):
    spec = importlib.util.spec_from_file_location(Path(relative).stem, ROOT / relative)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


def _directives(unit_text: str) -> set[str]:
    return {
        match.group(1)
        for match in re.finditer(r"^([A-Za-z][A-Za-z0-9]*)=", unit_text, re.MULTILINE)
    }


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


def _assert_runtime_directives(unit: str, backend: str, suffix: str) -> None:
    # M adds a main-process watchdog in both environments; production also has
    # explicit CPU limits. Every other directive still uses the backend sandbox.
    rust_directives = {"WatchdogSec", "NotifyAccess"}
    if suffix == "":
        rust_directives |= {"CPUQuota", "CPUWeight"}
    assert _directives(unit) - _directives(backend) <= rust_directives
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
def test_rust_unit_reuses_only_directives_the_running_backend_unit_already_uses(suffix):
    unit = _text(f"deploy/systemd/verdaxis-rust{suffix}.service")
    backend = _text(f"deploy/systemd/verdaxis-backend{suffix}.service")
    _assert_runtime_directives(unit, backend, suffix)
    for section in ("[Unit]", "[Service]", "[Install]"):
        assert section in unit
    for invalid in (
        unit + "\nNewRuntimeDirective=true\n",
        unit + "\nSetCredential=another-key:\n",
        unit + "\nImportCredential=*\n",
        unit.replace("WatchdogSec=60s", "WatchdogSec=0"),
        unit.replace("NotifyAccess=main", "NotifyAccess=all"),
    ):
        with pytest.raises(AssertionError):
            _assert_runtime_directives(invalid, backend, suffix)


def _staged_runtime_environment(tmp_path: Path) -> dict[str, Path]:
    """Stage the three deploy-written Rust files with the real helpers.

    ``deploy.sh`` runs as the deployment account under ``umask 077`` and the
    helpers create their outputs with explicit owner-only modes; this reproduces
    that from the same helper code the transaction invokes.
    """
    previous_umask = os.umask(0o077)
    try:
        environment_file = tmp_path / ".env"
        # The deployed staging `.env` shape: the budget helper loads it through
        # the real Settings class, so the fixture must satisfy the same
        # environment validators the live file does.
        environment_file.write_text(
            "ENVIRONMENT=staging\n"
            f"RELEASE_SHA={RELEASE_SHA}\n"
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
        runtime_dir = tmp_path / ".runtime-rust"
        runtime = _module(PREPARE_RUNTIME)
        result = runtime.prepare(
            environment_file,
            RELEASE_SHA,
            runtime_dir,
            ROOT,
            MIGRATION_REVISION,
        )
        budget = _module(PREPARE_BUDGET)
        budget_path = runtime_dir / "budget.json"
        budget.prepare(environment_file, RELEASE_SHA, budget_path)
        # The active binary is installed by deploy.sh's copy-chmod-rename
        # sequence, never a symlink into the source tree.
        staged = tmp_path / "releases" / RELEASE_SHA / "verdaxis-read"
        staged.parent.mkdir(parents=True)
        staged.write_bytes(b"#!/bin/sh\nexit 0\n")
        active = runtime_dir / "verdaxis-read"
        shutil.copyfile(staged, f"{active}.tmp")
        os.chmod(f"{active}.tmp", 0o755)
        os.rename(f"{active}.tmp", active)
        return {
            "runtime_dir": runtime_dir,
            "runtime_env": result["runtime_env"],
            "budget": budget_path,
            "active": active,
        }
    finally:
        os.umask(previous_umask)


def test_rust_runtime_files_are_owner_only_and_executable_by_the_service_user():
    """PM requirement 2: the unit's account owns and can read/run its files."""
    with tempfile.TemporaryDirectory() as directory:
        staged = _staged_runtime_environment(Path(directory))
        runtime_env = staged["runtime_env"]
        budget = staged["budget"]
        active = staged["active"]

        for path in (runtime_env, budget):
            mode = stat.S_IMODE(path.stat().st_mode)
            assert mode == 0o600, f"{path.name} must be owner-only"
            # No group or other bit: no other user can read the file.
            assert mode & 0o077 == 0
            assert mode & 0o400
            assert path.stat().st_uid == os.geteuid()
        assert stat.S_IMODE(staged["runtime_dir"].stat().st_mode) == 0o700
        assert stat.S_IMODE(active.stat().st_mode) == 0o755
        assert active.is_file() and not active.is_symlink()
        assert os.access(active, os.X_OK)
        assert "KYC_EXECUTION_GATE_ENABLED=false\n" in runtime_env.read_text()

        # The owner is the account the unit runs as and the account that runs
        # the deploy helper: one deployment identity, not two.
        assert f"User={STAGING_USER}\n" in _text(UNIT)
        readme = _text("README.md")
        assert f"sudo -n -u {STAGING_USER}" in readme
        assert STAGING_BACKEND == f"/home/{STAGING_USER}/verdaxis/staging/be"


def test_deploy_stages_rust_bytes_under_umask_077_and_installs_by_rename():
    deploy = _text(DEPLOY)
    region = deploy[
        deploy.index('RUST_EXTRACT_DIR="$RUST_RUNTIME_DIR/releases/$REMOTE_SHA"') :
        deploy.index("SERVICE_RESTART_ATTEMPTED=1")
    ]
    # The policy archive establishes the private umask before artifact extraction.
    policy = deploy.index('RUST_POLICY_TEMP="$(mktemp -d /tmp/verdaxis-rust-policy.')
    extraction = deploy.index('RUST_EXTRACT_DIR="$RUST_RUNTIME_DIR/releases/$REMOTE_SHA"')
    assert deploy.rindex("umask 077", 0, policy) < policy < extraction
    assert 'verify_rust_artifact_for_release "$REMOTE_SHA" "$RUST_EXTRACT_DIR"' in region
    steps = [
        'cp -- "$STAGED_BINARY" "$ACTIVE_BINARY.tmp.$$"',
        'chmod 0755 "$ACTIVE_BINARY.tmp.$$"',
        'verify_binary_digest "$ACTIVE_BINARY.tmp.$$"',
        'sync -f "$ACTIVE_BINARY.tmp.$$"',
        'mv -- "$ACTIVE_BINARY.tmp.$$" "$ACTIVE_BINARY"',
    ]
    positions = [region.index(step) for step in steps]
    assert positions == sorted(positions)
    assert "ln -s" not in region


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


def test_installer_requires_its_own_clean_checkout_at_the_source_ref():
    installer = _text(INSTALLER)
    assert '"$("${SOURCE_GIT[@]}" rev-parse HEAD)" != "$SOURCE_REF"' in installer
    assert "status --porcelain --untracked-files=all" in installer
    assert 'realpath -e "$SOURCE_ROOT"' in installer
    assert "/home/verdaxis-prod/verdaxis/staging/be" in installer
    assert "systemd-analyze verify" in installer
    assert "systemctl start" not in installer
    assert "systemctl restart" not in installer
    # Unit selection stays manifest-driven: no Rust unit name in installer code.
    assert RUST_UNIT_NAME not in installer
    assert f"staging\t{RUST_UNIT_NAME}" in _text(MANIFEST).splitlines()


def _assert_staging_credential_boundary(unit: str) -> None:
    assert "/home/verdaxis-prod/verdaxis/prod/be" not in unit
    assert "MIGRATOR_DATABASE_URL" not in unit
    assert "/etc/verdaxis/kyc-document-keys.json" not in unit
    _assert_runtime_directives(unit, _text(BACKEND_STAGING_UNIT), "-staging")


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
