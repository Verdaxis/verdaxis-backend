"""Exercise the selective installer without systemd, root, network or Cargo."""
from __future__ import annotations

import hashlib
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "deploy/external_monitor"


@pytest.fixture
def host(tmp_path):
    tools = tmp_path / "tools"
    tools.mkdir()
    calls = tmp_path / "calls"
    calls.touch()
    active = tmp_path / "active"
    mask = tmp_path / "mask"
    mask.write_text("masked")

    def tool(name, text):
        path = tools / name
        path.write_text("#!/bin/bash\nset -eu\n" + text)
        path.chmod(0o755)

    tool("id", "printf '0\\n'\n")
    tool("systemctl", f"""printf '%s\\n' "$*" >> '{calls}'
case $1 in
 show) if [[ -f '{active}' ]]; then printf 'active\\n'; else printf 'inactive\\n'; fi ;;
 is-enabled) /bin/cat '{mask}' ;;
 daemon-reload) ;;
 *) exit 99 ;;
esac
""")
    # Keep real install/cmp/hash/stat/mv operations. Only ownership is simulated.
    tool("install", f'''args=()
while (( $# )); do
    case $1 in -o|-g) shift 2 ;; *) args+=("$1"); shift ;; esac
done
exec {shutil.which("install")} "${{args[@]}}"
''')
    tool("stat", f"""mode=$({shutil.which('stat')} -c '%a' "${{@: -1}}")
printf '0:0:%s\\n' "$mode"
""")
    script = tmp_path / "install.sh"
    text = (SOURCE / "install.sh").read_text()
    for prefix in ("/usr/local/libexec/verdaxis", "/etc/systemd/system", "/var/lib/verdaxis-monitor-install"):
        text = text.replace(prefix, str(tmp_path / prefix.lstrip("/")))
    script.write_text(text)
    shutil.copytree(SOURCE / "systemd", tmp_path / "systemd")
    binary = tmp_path / "release-binary"
    binary.write_bytes(b"fixture CI binary\n")
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    paths = [tmp_path / "usr/local/libexec/verdaxis/verdaxis-read-monitor"] + [
        tmp_path / "etc/systemd/system" / name
        for name in ("verdaxis-monitor.service", "verdaxis-monitor-verify.service")
    ]

    def run(*args):
        return subprocess.run(
            ["bash", str(script), *map(str, args)],
            env=dict(os.environ, PATH=f"{tools}:{os.environ['PATH']}"),
            capture_output=True, text=True,
        )

    return tmp_path, paths, binary, digest, run, active, mask, calls


def test_install_repeat_and_rollback_preserve_original_files(host):
    root, paths, binary, digest, run, _, _, calls = host
    for index, path in enumerate(paths):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"old-{index}\n")
        path.chmod(0o640)
    first = run("install", binary, digest)
    assert first.returncode == 0, first.stderr
    stamp = re.search(r"Rollback key: (\S+)", first.stdout)[1]
    assert paths[0].read_bytes() == binary.read_bytes()
    for index, path in enumerate(paths):
        assert Path(f"{path}.bak.{stamp}").read_text() == f"old-{index}\n"
        assert path.stat().st_mode & 0o777 == (0o755 if index == 0 else 0o644)
    again = run("install", binary, digest)
    assert again.returncode == 0, again.stderr
    assert "Already installed" in again.stdout
    assert len(list((root / "var/lib/verdaxis-monitor-install").glob("*Z"))) == 1
    for _ in range(2):
        restored = run("rollback", stamp)
        assert restored.returncode == 0, restored.stderr
        for index, path in enumerate(paths):
            assert path.read_text() == f"old-{index}\n"
            assert path.stat().st_mode & 0o777 == 0o640
    assert calls.read_text().count("daemon-reload") == 4


def test_first_install_rollback_removes_only_new_files(host):
    _, paths, binary, digest, run, _, _, _ = host
    result = run("install", binary, digest)
    assert result.returncode == 0, result.stderr
    stamp = re.search(r"Rollback key: (\S+)", result.stdout)[1]
    assert run("rollback", stamp).returncode == 0
    assert all(not path.exists() for path in paths)


@pytest.mark.parametrize("reason", ["digest", "active", "mask", "symlink"])
def test_refusals_do_not_replace_files(host, reason):
    root, paths, binary, digest, run, active, mask, calls = host
    paths[1].parent.mkdir(parents=True)
    paths[1].write_text("installed python unit\n")
    if reason == "digest":
        digest = "0" * 64
    elif reason == "active":
        active.touch()
    elif reason == "mask":
        mask.write_text("disabled")
    else:
        paths[0].parent.mkdir(parents=True)
        paths[0].symlink_to(binary)
    result = run("install", binary, digest)
    assert result.returncode != 0
    assert paths[1].read_text() == "installed python unit\n"
    assert "daemon-reload" not in calls.read_text()
    assert not list(root.rglob("*.bak.*"))


def test_units_use_dedicated_binary_and_do_not_start_recovery():
    for name, suffix in (("verdaxis-monitor.service", ""), ("verdaxis-monitor-verify.service", " --telegram-disabled")):
        unit = (SOURCE / "systemd" / name).read_text()
        assert f"ExecStart=/usr/local/libexec/verdaxis/verdaxis-read-monitor monitor external{suffix}\n" in unit
        assert "EnvironmentFile=-/etc/verdaxis-monitor.env\n" in unit
        assert "OnFailure=" not in unit
    assert "ExecStart=/usr/local/sbin/verdaxis-recover\n" in (SOURCE / "systemd/verdaxis-recover.service").read_text()
    assert "ExecStart=/usr/local/sbin/verdaxis-codex-diagnose\n" in (SOURCE / "systemd/verdaxis-codex-diagnose.service").read_text()
