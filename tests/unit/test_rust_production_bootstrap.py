"""Real production bootstrap analyzer checks; no artifact or service starts."""
import os
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
BASE = "/home/verdaxis-prod/verdaxis/prod/be"
UNIT = "deploy/systemd/verdaxis-rust.service"


@pytest.mark.parametrize("caller", ["old-helper", "installer", "next-helper"])
def test_production_bootstrap_verify_without_runtime_directory(tmp_path, caller):
    if not shutil.which("systemd-analyze"):
        pytest.skip("Linux analyzer required")
    version = subprocess.run(["systemd-analyze", "--version"], capture_output=True, text=True, check=True).stdout.splitlines()[0]
    if caller == "old-helper":
        # Production baseline 56d75770 uses this exact command shape.
        source = subprocess.run(["git", "show", "56d7577085af99411a6f9c3e34e64ad929d09954:scripts/deploy.sh"], cwd=ROOT, capture_output=True, text=True, check=True).stdout
    else:
        source = (ROOT / ("scripts/install_systemd_units.sh" if caller == "installer" else "scripts/deploy.sh")).read_text()
    command = next(line.strip() for line in source.splitlines() if "systemd-analyze verify" in line).removeprefix("sudo ")
    units = tmp_path / "units"
    units.mkdir()
    backend = tmp_path / "be"
    python = backend / "venv/bin/python"
    python.parent.mkdir(parents=True)
    python.symlink_to(sys.executable)
    unit = units / "verdaxis-rust.service"
    unit.write_text((ROOT / UNIT).read_text().replace(BASE, str(backend)))
    (units / "postgresql.service").write_text("[Service]\nType=oneshot\nExecStart=/bin/true\n")
    for target in ("sysinit.target", "basic.target", "network-online.target", "shutdown.target"):
        (units / target).write_text("[Unit]\nDescription=fixture target\n")
    assert not (backend / ".runtime-rust").exists()
    result = subprocess.run(
        ["bash", "-c", 'CANDIDATE_UNITS=("$UNIT_PATH"); UNIT_SOURCES=("$UNIT_PATH"); ' + command],
        env={**os.environ, "SYSTEMD_UNIT_PATH": str(units), "UNIT_PATH": str(unit)},
        capture_output=True, text=True, timeout=30,
    )
    print(f"{version}: {caller} VERIFY_EXIT={result.returncode}\n{result.stderr}")
    assert result.returncode == 0, result.stderr
    assert not (backend / ".runtime-rust").exists()


def test_production_env_launcher_preserves_pid_and_release(tmp_path):
    command = next(line.removeprefix("ExecStart=") for line in (ROOT / UNIT).read_text().splitlines() if line.startswith("ExecStart="))
    arguments = shlex.split(command)
    assert arguments[:2] == ["/usr/bin/env", BASE + "/.runtime-rust/verdaxis-read"]
    assert arguments[2:] == ["--bind", "127.0.0.1:8010", "--budget-file", BASE + "/.runtime-rust/budget.json"]
    process = subprocess.Popen([arguments[0], sys.executable, "-c", "import os; print(os.getpid(), os.environ['RELEASE_SHA'])"], env={**os.environ, "RELEASE_SHA": "a" * 40}, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    stdout, stderr = process.communicate(timeout=10)
    assert process.returncode == 0, stderr
    assert stdout.strip() == f"{process.pid} {'a' * 40}"
