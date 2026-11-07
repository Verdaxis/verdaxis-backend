"""Task 33 fixture contract: post-deletion evidence checker.

Exercises deploy/tests/post-deletion-contract.sh in fixture mode only. A
throwaway RSA key pair is generated per session; each fixture's inventory is
signed with it so the checker's openssl verification path runs. Every bad-*
set must fail exactly its own check. This proves the checker only: no host is
contacted and no installed evidence is collected.
"""
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "deploy/tests/post-deletion-contract.sh"
FIXTURES = ROOT / "deploy/tests/fixtures/post-deletion"

CHECKS = [
    "host_inventory_is_complete_and_signed",
    "no_python_process_unit_timer_or_cron_on_either_host",
    "backup_and_staging_pull_schedules_are_unchanged",
    "passive_harm_checks_are_clean_for_each_group",
    "independent_review_findings_are_closed",
    "removal_ledger_has_no_gate_or_ownerless_rows",
    "deleted_paths_are_absent_on_both_hosts",
    "the_rust_backup_verify_monitor_accepts_the_nightly_artifact",
]


def openssl(*arguments):
    subprocess.run(["openssl", *arguments], check=True, capture_output=True)


@pytest.fixture(scope="session")
def signing_key(tmp_path_factory):
    """A throwaway RSA key pair for the signature checks."""
    directory = tmp_path_factory.mktemp("post-deletion-key")
    private_key = directory / "fixture-signing.pem"
    public_key = directory / "fixture-signing.pub"
    openssl("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
            "-out", str(private_key))
    openssl("pkey", "-in", str(private_key), "-pubout", "-out", str(public_key))
    return private_key, public_key


@pytest.fixture(scope="session")
def workdir(tmp_path_factory):
    return tmp_path_factory.mktemp("post-deletion-run")


def run_checker(fixture_dir, private_key, public_key, workdir):
    """Run the checker on a fresh copy of one fixture set.

    The detached signature binds the exact inventory bytes, so it is made over
    the staged copy. Returns (exit_code, stdout, stderr).
    """
    staged = workdir / fixture_dir.name
    if staged.exists():
        shutil.rmtree(staged)
    shutil.copytree(fixture_dir, staged)
    openssl("dgst", "-sha256", "-sign", str(private_key), "-binary",
            "-out", str(staged / "inventory.json.sig"), str(staged / "inventory.json"))
    result = subprocess.run(
        ["bash", str(SCRIPT), "--fixture", str(staged), "--public-key", str(public_key)],
        capture_output=True, text=True)
    return result.returncode, result.stdout, result.stderr


def test_checker_script_exists_and_is_shellcheck_registered():
    assert SCRIPT.is_file(), "missing deploy/tests/post-deletion-contract.sh"
    reviewed = (ROOT / "deploy/tests/shellcheck-files.txt").read_text().splitlines()
    assert "deploy/tests/post-deletion-contract.sh" in reviewed, (
        "checker script is not in the reviewed shellcheck list"
    )


def test_good_fixture_passes_every_check(signing_key, workdir):
    private_key, public_key = signing_key
    exit_code, output, _ = run_checker(FIXTURES / "good", private_key, public_key, workdir)
    assert exit_code == 0, f"good fixture failed: {output}"
    reported = [line.split()[1] for line in output.splitlines() if line.startswith("PASS ")]
    assert reported == CHECKS, f"expected the eight named checks in order, got {reported}"


@pytest.mark.parametrize("check", CHECKS)
def test_each_bad_fixture_fails_only_its_own_check(check, signing_key, workdir):
    private_key, public_key = signing_key
    fixture = FIXTURES / f"bad-{check}"
    assert fixture.is_dir(), f"missing fixture set {fixture}"
    exit_code, output, _ = run_checker(fixture, private_key, public_key, workdir)
    assert exit_code != 0, f"bad-{check} unexpectedly passed: {output}"
    failing_names = [
        line[len("FAIL "):].split(":", 1)[0]
        for line in output.splitlines() if line.startswith("FAIL ")
    ]
    assert failing_names == [check], (
        f"bad-{check} must fail exactly its own check, got {failing_names}"
    )


def test_missing_public_key_refuses(signing_key, workdir):
    private_key, _ = signing_key
    exit_code, _, errors = run_checker(
        FIXTURES / "good", private_key, workdir / "absent.pem", workdir)
    assert exit_code != 0
    assert "missing public key" in errors


def test_wrong_key_fails_the_signature_check(signing_key, workdir):
    """Inventory signed by one key must not verify under a different key."""
    _, trusted_public = signing_key
    other_private = workdir / "other.pem"
    other_public = workdir / "other.pub"
    openssl("genpkey", "-algorithm", "RSA", "-pkeyopt", "rsa_keygen_bits:2048",
            "-out", str(other_private))
    openssl("pkey", "-in", str(other_private), "-pubout", "-out", str(other_public))
    # Sign the staged inventory with the other key, but keep verifying with
    # the trusted public key: the signature must then refuse.
    fixture = FIXTURES / "good"
    staged = workdir / "wrong-key"
    if staged.exists():
        shutil.rmtree(staged)
    shutil.copytree(fixture, staged)
    openssl("dgst", "-sha256", "-sign", str(other_private), "-binary",
            "-out", str(staged / "inventory.json.sig"), str(staged / "inventory.json"))
    result = subprocess.run(
        ["bash", str(SCRIPT), "--fixture", str(staged), "--public-key", str(trusted_public)],
        capture_output=True, text=True)
    assert result.returncode != 0
    assert "FAIL host_inventory_is_complete_and_signed" in result.stdout
