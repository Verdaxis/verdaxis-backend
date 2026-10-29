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

from tests.unit.test_rust_release import RustReleaseFixture

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
        ).removeprefix("sudo ")

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


def test_rust_start_preserves_environment_arguments_and_main_pid(tmp_path):
    import shlex
    import sys

    backend = tmp_path / "be"
    binary = backend / ".runtime-rust/verdaxis-read"
    binary.parent.mkdir(parents=True)
    binary.symlink_to(sys.executable)
    command = next(
        line.removeprefix("ExecStart=") for line in _text(UNIT).splitlines()
        if line.startswith("ExecStart=")
    ).replace(STAGING_BACKEND, str(backend))
    arguments = shlex.split(command)
    assert arguments[2:] == [
        "--bind", BIND, "--budget-file", str(binary.parent / "budget.json")
    ]
    # Use a real executable to observe exec semantics, without starting a server.
    process = subprocess.Popen(
        arguments[:2] + ["-c", "import os; print(os.getpid(), os.environ['RELEASE_SHA'])"],
        env={**os.environ, "RELEASE_SHA": RELEASE_SHA},
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    stdout, stderr = process.communicate(timeout=10)
    assert process.returncode == 0, stderr
    assert stdout.strip() == f"{process.pid} {RELEASE_SHA}"


def test_rust_unit_keeps_runtime_credentials_separate():
    """The plan's unit contract: loopback bind, bounded memory, no `.env`."""
    unit = _text(UNIT)
    assert f"--bind {BIND}" in unit
    assert "MemoryMax=256M" in unit
    assert f"EnvironmentFile={STAGING_BACKEND}/.env\n" not in unit
    assert "verify_deploy_start_guard.py" in unit
    assert "verify_rust_start.py" in unit


def test_rust_unit_paths_are_the_files_the_deploy_transaction_writes():
    """PM requirement 1: unit paths and deploy paths are one selection."""
    unit = _text(UNIT)
    deploy = _text(DEPLOY)

    # The runtime directory, the active binary and the budget artifact, exactly
    # as the transaction names them.
    assert f"RUST_UNIT_NAME=\"{RUST_UNIT_NAME}\"" in deploy
    assert 'RUST_RUNTIME_DIR="$BACKEND_DIR/.runtime-rust"' in deploy
    assert 'ACTIVE_BINARY="$RUST_RUNTIME_DIR/verdaxis-read"' in deploy
    assert '--output "$RUST_RUNTIME_DIR/budget.json"' in deploy
    assert 'target = output_dir / "runtime.env"' in _text(PREPARE_RUNTIME)

    assert f"WorkingDirectory={STAGING_BACKEND}\n" in unit
    assert f"ExecStart=/usr/bin/env {RUNTIME_DIR}/verdaxis-read --bind {BIND} " in unit
    assert f"--budget-file {RUNTIME_DIR}/budget.json\n" in unit
    # Every EnvironmentFile is a deploy-written file; the unit never hands the
    # secret-rich `.env` to systemd.
    assert re.findall(r"^EnvironmentFile=(.*)$", unit, re.MULTILINE) == [
        f"{STAGING_BACKEND}/.runtime-release.env",
        f"{RUNTIME_DIR}/runtime.env",
    ]


def test_rust_unit_budget_check_uses_the_prepared_artifact_and_real_flags():
    """The prestart budget check is the drift gate the transaction prepared."""
    unit = _text(UNIT)
    budget_help = _text(PREPARE_BUDGET)

    assert "verify_rust_start.py --expected-release-sha ${RELEASE_SHA}" in unit
    assert f"--budget-file {RUNTIME_DIR}/budget.json" in unit
    assert "--environment-file" not in unit
    for flag in ("--environment-file", "--release-sha", "--output", "--verify"):
        assert flag in budget_help
    assert unit.index("verify_rust_start.py") < unit.index("ExecStart=")



def test_rust_unit_bind_and_service_name_match_the_committed_policy():
    policy = json.loads(_text(POLICY))
    unit = _text(UNIT)

    assert Path(UNIT).name == policy["service_name"] == RUST_UNIT_NAME
    assert policy["bind"] == BIND
    assert policy["health_url"] == f"http://{BIND}/health/ready"
    assert f"--bind {policy['bind']}" in unit


def _assert_runtime_directives(unit: str, backend: str, suffix: str) -> None:
    assert _directives(unit) <= _directives(backend)
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
    for extra in ("NewRuntimeDirective=true", "SetCredential=another-key:", "ImportCredential=*"):
        with pytest.raises(AssertionError):
            _assert_runtime_directives(unit + "\n" + extra + "\n", backend, suffix)


def test_rust_unit_runs_the_start_guard_and_the_revision_check():
    unit = _text(UNIT)
    assert (
        "ExecStartPre=/usr/bin/env -i /usr/bin/python3 -I -S -B "
        f"{STAGING_BACKEND}/scripts/verify_deploy_start_guard.py "
        "--environment staging --expected-release-sha ${RELEASE_SHA}\n"
    ) in unit
    assert (
        "ExecStartPre=/usr/bin/env -i /usr/bin/python3 -I -S -B "
        f"{STAGING_BACKEND}/scripts/verify_rust_start.py "
        "--expected-release-sha ${RELEASE_SHA} "
        f"--budget-file {RUNTIME_DIR}/budget.json --expected-revision ${{MIGRATION_REVISION}} "
        f"--revision-file {RUNTIME_DIR}/revision.json\n"
    ) in unit
    assert f"User={STAGING_USER}\n" in unit
    assert f"Group={STAGING_USER}\n" in unit


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
    assert region.index("umask 077") < region.index('mkdir -p -- "$RUST_RUNTIME_DIR"')
    assert 'chmod 0755 "$ACTIVE_BINARY.tmp.$$"' in region
    assert 'mv -- "$ACTIVE_BINARY.tmp.$$" "$ACTIVE_BINARY"' in region
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


def test_bootstrap_release_deploys_without_the_rust_unit_or_an_artifact(tmp_path):
    """Release one: helper and artifact support land with Rust inactive."""
    fixture = RustReleaseFixture(tmp_path)
    result = fixture.deploy(fixture.old_sha)

    assert result.returncode == 0, result.stderr
    executed = fixture.ledger
    assert not any("rust" in entry.lower() for entry in executed)
    assert [e for e in executed if e.startswith("sudo:systemctl restart")] == [
        "sudo:systemctl restart verdaxis-backend-staging-test.service"
    ]
    # No artifact was offered and none was required.
    assert "RUST_ARTIFACT_ARCHIVE" not in fixture.deploy_environment(fixture.old_sha)
    assert not (fixture.checkout / ".runtime-rust").exists()


def test_activation_requires_the_manifest_unit_and_a_verified_artifact(tmp_path):
    """Release two: both committed records plus a verified artifact."""
    fixture = RustReleaseFixture(tmp_path)
    enabled_sha = fixture.push_rust_release(enabled=True)
    fixture.build_artifact(enabled_sha)

    # Without the artifact the transaction refuses before any service stops.
    without_artifact = fixture.deploy_environment(enabled_sha)
    without_artifact["RUST_ARTIFACT_ARCHIVE"] = ""
    refused = subprocess.run(
        ["bash", "scripts/deploy.sh"],
        cwd=fixture.checkout,
        env=without_artifact,
        capture_output=True,
        text=True,
        check=False,
    )
    assert refused.returncode != 0
    assert not fixture.stopped_services()

    activated = fixture.deploy(enabled_sha)
    assert activated.returncode == 0, activated.stderr
    assert [
        e for e in fixture.ledger if e.startswith("sudo:systemctl restart")
    ] == [
        "sudo:systemctl restart verdaxis-backend-staging-test.service",
        f"sudo:systemctl restart {RUST_UNIT_NAME}"
    ]
    assert (fixture.checkout / ".runtime-rust/verdaxis-read").is_file()
    assert fixture.state_text() == ""


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
    _assert_runtime_directives(unit, _text(BACKEND_STAGING_UNIT))


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


def test_exact_rust_prestarts_need_no_app_environment(tmp_path):
    import shlex
    backend = tmp_path / "be"
    (backend / "scripts").mkdir(parents=True)
    for name in ("verify_deploy_start_guard.py", "verify_rust_start.py"):
        shutil.copy2(ROOT / "scripts" / name, backend / "scripts" / name)
    runtime = backend / ".runtime-rust"
    runtime.mkdir()
    budget = runtime / "budget.json"
    budget.write_text(json.dumps({"version": 1, "release_sha": RELEASE_SHA}))
    budget.chmod(0o600)
    revision = runtime / "revision.json"
    revision.write_text(json.dumps({"release_sha": RELEASE_SHA, "revision": MIGRATION_REVISION}))
    revision.chmod(0o600)
    # Exactly the unit prestarts, relocating only the fixed checkout path and
    # substituting systemd's release variable. -I -S excludes site packages.
    for line in _text(UNIT).splitlines():
        if not line.startswith("ExecStartPre="): continue
        command = line.removeprefix("ExecStartPre=").replace(STAGING_BACKEND, str(backend)).replace("${RELEASE_SHA}", RELEASE_SHA).replace("${MIGRATION_REVISION}", MIGRATION_REVISION)
        result = subprocess.run(shlex.split(command), cwd=backend, env={"ENVIRONMENT": "staging", "RELEASE_SHA": RELEASE_SHA}, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
    assert not (backend / ".env").exists()
    # Tampered checkpoint evidence is a hard refusal with the same unit command.
    revision.write_text(json.dumps({"release_sha": RELEASE_SHA, "revision": "wrong"}))
    refused = subprocess.run(shlex.split(command), cwd=backend,
                             env={"RELEASE_SHA": RELEASE_SHA}, capture_output=True, text=True)
    assert refused.returncode == 1


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
