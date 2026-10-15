"""Execute the installer's metadata-only preflight without host access."""
import os
import stat
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest

INSTALLER = Path("scripts/install_systemd_units.sh")


def preflight_source():
    source = INSTALLER.read_text()
    marker = "<<'KYC_KEY_SOURCE_PREFLIGHT'\n"
    assert marker in source, "Installer must check KYC source metadata"
    return source.split(marker, 1)[1].split("\nKYC_KEY_SOURCE_PREFLIGHT", 1)[0]


@pytest.mark.parametrize("environment", ["staging", "production"])
@pytest.mark.parametrize("problem", [None, "missing", "missing_parent", "mode", "owner", "group", "symlink", "fifo", "directory", "parent_write", "parent_owner", "parent_symlink"])
def test_source_metadata_policy(monkeypatch, capsys, environment, problem):
    code = preflight_source()
    suffix = "-staging" if environment == "staging" else ""
    target = f"/etc/verdaxis/kyc-document-keys{suffix}.json"
    def metadata(path):
        path = str(path)
        if path == target:
            if problem in {"missing", "missing_parent"}:
                raise FileNotFoundError
            mode = stat.S_IFREG | 0o400
            if problem == "mode": mode = stat.S_IFREG | 0o644
            if problem == "symlink": mode = stat.S_IFLNK | 0o777
            if problem == "fifo": mode = stat.S_IFIFO | 0o400
            if problem == "directory": mode = stat.S_IFDIR | 0o400
            return SimpleNamespace(st_mode=mode, st_uid=int(problem == "owner"), st_gid=int(problem == "group"))
        assert path in {"/etc/verdaxis", "/etc", "/"}
        if path == "/etc/verdaxis" and problem == "missing_parent":
            raise FileNotFoundError
        mode = stat.S_IFDIR | 0o755
        uid = 0
        if path == "/etc/verdaxis":
            if problem == "parent_write": mode |= 0o020
            if problem == "parent_owner": uid = 1000
            if problem == "parent_symlink": mode = stat.S_IFLNK | 0o777
        return SimpleNamespace(st_mode=mode, st_uid=uid, st_gid=0)
    monkeypatch.setattr(os, "lstat", metadata)
    monkeypatch.setattr(os, "geteuid", lambda: 0)
    monkeypatch.setattr(sys, "argv", ["-", environment])
    if problem in {None, "missing", "missing_parent"}:
        exec(compile(code, "preflight", "exec"), {"__name__": "__main__"})
    else:
        with pytest.raises(SystemExit) as error:
            exec(compile(code, "preflight", "exec"), {"__name__": "__main__"})
        assert error.value.code == 1
        assert "KYC key source metadata is unsafe" in capsys.readouterr().err


def test_preflight_precedes_dry_run_and_mutation():
    source = INSTALLER.read_text()
    assert source.index("KYC_KEY_SOURCE_PREFLIGHT") < source.index('if [[ "$MODE" == "dry-run" ]]')
    assert source.index("KYC_KEY_SOURCE_PREFLIGHT") < source.index('PENDING_DIR="/var/lib')
    assert 'sudo /usr/bin/python3 -I -S -B - "$DEPLOY_ENVIRONMENT"' in source
    subprocess.run(["/bin/bash", "-n", str(INSTALLER)], check=True)
