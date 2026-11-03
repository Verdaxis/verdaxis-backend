"""The installer's metadata-only KYC key-source preflight, now shell.

The check moved from inline Python to shell (Phase 10 Task 10). The cases run
in deploy/tests/units-contract.sh against the installer's exact lines with a
fake `sudo stat`; these tests run that contract once and require each case.
"""
import subprocess
from functools import lru_cache
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "deploy/tests/units-contract.sh"


@lru_cache(maxsize=1)
def contract_cases():
    result = subprocess.run(
        ["bash", str(CONTRACT)], capture_output=True, text=True, timeout=60, check=False
    )
    assert result.returncode == 0, result.stderr
    return {line.removeprefix("ok ") for line in result.stdout.splitlines() if line.startswith("ok ")}


@pytest.mark.parametrize("environment", ["staging", "production"])
@pytest.mark.parametrize("problem", [None, "missing", "missing_parent", "mode", "owner", "group", "symlink", "fifo", "directory", "parent_write", "parent_owner", "parent_symlink"])
def test_source_metadata_policy(environment, problem):
    assert f"kyc_source_metadata_policy/{environment}/{problem or 'none'}" in contract_cases()


def test_preflight_precedes_dry_run_and_mutation():
    assert "kyc_preflight_precedes_dry_run_and_mutation" in contract_cases()


def test_absent_source_is_refused_with_the_placeholder_command():
    """An absent key source must stop the installer, not pass silently."""
    assert "kyc_absent_source_is_refused_with_the_placeholder_command" in contract_cases()


def test_missing_etc_verdaxis_is_refused_with_the_placeholder_command():
    """No /etc/verdaxis at all is still an absent source, refused by name."""
    assert "kyc_missing_etc_verdaxis_is_refused_with_the_placeholder_command" in contract_cases()


def test_preflight_never_reads_key_contents():
    """Only metadata is inspected; the preflight never opens the key file."""
    assert "kyc_preflight_never_reads_key_contents" in contract_cases()
