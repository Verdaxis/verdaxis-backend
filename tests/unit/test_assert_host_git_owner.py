"""The assertion helper must run git as the checkout owner, never as root.

Root git calls against /home/verdaxis-prod/verdaxis/prod/be fail with "detected
dubious ownership", and a root safe.directory would let root read a repository
the service user can write. Every git call is routed through
sudo -n -u <owner> with the clean environment kept.
"""
import hashlib
import json
import socket
from pathlib import Path
from types import SimpleNamespace

import pytest

from scripts.phase0 import assert_host

OWNER_NAME = "verdaxis-prod"
OWNER_UID = 1200


class StubCommand:
    """Record subprocess argv and answer the two scripted git calls."""

    def __init__(self, head, status=""):
        self.head = head
        self.status = status
        self.calls = []

    def __call__(self, arguments):
        self.calls.append(list(arguments))
        if arguments[-2:] == ["rev-parse", "HEAD"]:
            return self.head
        if arguments[-1] == "--untracked-files=no":
            return self.status
        raise AssertionError(f"unexpected command: {arguments}")

    def git_calls(self):
        return [call for call in self.calls if "/usr/bin/git" in call]


REAL_CHECKOUT_OWNER = assert_host.checkout_owner


def patch_checkout_owner(monkeypatch, uid, name=OWNER_NAME):
    monkeypatch.setattr(assert_host, "checkout_owner",
                        lambda: REAL_CHECKOUT_OWNER(
                            stat_path=lambda path: SimpleNamespace(st_uid=uid),
                            lookup_user=lambda value: SimpleNamespace(pw_name=name)))


@pytest.fixture
def tuple_state(tmp_path, monkeypatch):
    sha = "a" * 40
    identity = f"ENVIRONMENT=production\nRELEASE_SHA={sha}\n"
    (tmp_path / ".runtime-monitor.env").write_text(identity)
    (tmp_path / ".runtime-release.env").write_text(identity + "MIGRATION_REVISION=p0_20261016_union\n")
    (tmp_path / ".env").write_bytes(b"synthetic")
    # The unit directory is a fixture path too: the assertion helper reads
    # whatever UNIT_DIR names, never the real host's /etc/systemd/system.
    units_dir = tmp_path / "systemd"
    units = {
        units_dir / "verdaxis-backend.service": b"unit",
        units_dir / "verdaxis-backend.service.d/90-phase0-fence.conf": b"guard",
    }
    for path, value in units.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(value)
    monkeypatch.setattr(assert_host, "UNIT_DIR", units_dir)
    artifacts = {
        str(path): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in tmp_path.iterdir()
        if path.is_file()
    }
    artifacts.update({str(path): hashlib.sha256(value).hexdigest() for path, value in units.items()})
    manifest = dict(release_sha=sha, identity={"revision": "p0_20261016_union"},
                    monitor_identity_file=str(tmp_path / ".runtime-monitor.env"), artifacts=artifacts)
    return sha, manifest, units


def test_tuple_git_commands_run_as_the_checkout_owner(monkeypatch, tuple_state):
    sha, manifest, _ = tuple_state
    root = Path(manifest["monitor_identity_file"]).parent
    stub = StubCommand(sha)
    monkeypatch.setattr(assert_host, "ROOT", root)
    monkeypatch.setattr(assert_host, "command", stub)
    patch_checkout_owner(monkeypatch, OWNER_UID)
    assert_host.verify_tuple(manifest)
    git_calls = stub.git_calls()
    assert len(git_calls) == 2
    for call in git_calls:
        assert call[:5] == ["/usr/bin/sudo", "-n", "-u", OWNER_NAME, "/usr/bin/git"]
        assert call[5:7] == ["-C", str(root)]
        assert call[7:] in (["rev-parse", "HEAD"],
                            ["status", "--porcelain", "--untracked-files=no"])
    # No call may run git as root.
    assert not [call for call in stub.calls if call[0] == "/usr/bin/git"]


def test_root_owned_checkout_refuses_before_git(monkeypatch, tuple_state):
    _, manifest, _ = tuple_state
    stub = StubCommand("a" * 40)
    monkeypatch.setattr(assert_host, "ROOT", Path(manifest["monitor_identity_file"]).parent)
    monkeypatch.setattr(assert_host, "command", stub)
    patch_checkout_owner(monkeypatch, 0)
    with pytest.raises(ValueError, match="[Oo]wner"):
        assert_host.verify_tuple(manifest)
    assert stub.calls == []


def test_owner_lookup_failure_refuses(monkeypatch, tuple_state):
    _, manifest, _ = tuple_state
    stub = StubCommand("a" * 40)
    monkeypatch.setattr(assert_host, "ROOT", Path(manifest["monitor_identity_file"]).parent)
    monkeypatch.setattr(assert_host, "command", stub)

    def missing(path):
        raise FileNotFoundError(path)

    monkeypatch.setattr(assert_host, "checkout_owner",
                        lambda: REAL_CHECKOUT_OWNER(stat_path=missing))
    with pytest.raises(ValueError, match="[Oo]wner"):
        assert_host.verify_tuple(manifest)
    assert stub.calls == []


def test_unresolvable_owner_uid_refuses(monkeypatch, tuple_state):
    _, manifest, _ = tuple_state
    stub = StubCommand("a" * 40)
    monkeypatch.setattr(assert_host, "ROOT", Path(manifest["monitor_identity_file"]).parent)
    monkeypatch.setattr(assert_host, "command", stub)

    def unknown(value):
        raise KeyError(value)

    monkeypatch.setattr(assert_host, "checkout_owner",
                        lambda: REAL_CHECKOUT_OWNER(
                            stat_path=lambda path: SimpleNamespace(st_uid=OWNER_UID),
                            lookup_user=unknown))
    with pytest.raises(ValueError, match="[Oo]wner"):
        assert_host.verify_tuple(manifest)
    assert stub.calls == []


def test_readiness_check_keeps_owner_git_argv(monkeypatch, tuple_state):
    sha, manifest, _ = tuple_state
    stub = StubCommand(sha)
    monkeypatch.setattr(assert_host, "ROOT", Path(manifest["monitor_identity_file"]).parent)
    monkeypatch.setattr(assert_host, "command", stub)
    patch_checkout_owner(monkeypatch, OWNER_UID)

    class Response:
        def __init__(self, payload):
            self.payload = payload

        def read(self, limit=-1):
            return json.dumps(self.payload).encode()

        def __enter__(self):
            return self

        def __exit__(self, *exception):
            return False

    payload = dict(status="ok", db="ok", environment="production", release_sha=sha)
    opener = type("Opener", (), {"open": staticmethod(lambda url, timeout: Response(payload))})()
    monkeypatch.setattr(assert_host.urllib.request, "build_opener", lambda *args, **kwargs: opener)
    assert_host.run_check(dict(manifest, hostname=socket.gethostname().split(".")[0]), "readiness")
    for call in stub.git_calls():
        assert call[:5] == ["/usr/bin/sudo", "-n", "-u", OWNER_NAME, "/usr/bin/git"]
    assert not [call for call in stub.calls if call[0] == "/usr/bin/git"]
