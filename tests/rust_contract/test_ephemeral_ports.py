"""Kernel-owned ephemeral range discovery; no host settings are changed."""
from pathlib import Path
from types import SimpleNamespace

import pytest

from scripts.rust_contract import run_disposable as runner


@pytest.mark.parametrize("platform, text, expected", [
    ("linux", "32768\t60999\n", (32768, 60999)),
    ("darwin", "49152\n65535\n", (49152, 65535)),
])
def test_platform_range_parsing(monkeypatch, platform, text, expected):
    monkeypatch.setattr(runner.sys, "platform", platform)
    if platform == "linux":
        def read(path):
            assert str(path) == "/proc/sys/net/ipv4/ip_local_port_range"
            return text
        monkeypatch.setattr(Path, "read_text", read)
    else:
        def run(args, **kwargs):
            assert args == ["sysctl", "-n", "net.inet.ip.portrange.first", "net.inet.ip.portrange.last"]
            return SimpleNamespace(stdout=text)
        monkeypatch.setattr(runner.subprocess, "run", run)
    assert runner.ephemeral_range() == expected


@pytest.mark.parametrize("text", ["", "32768", "1 2 3", "bad 65535", "0 65535", "65535 32768", "32768 65536"])
def test_invalid_range_fails_closed(monkeypatch, text):
    monkeypatch.setattr(runner.sys, "platform", "linux")
    monkeypatch.setattr(Path, "read_text", lambda _: text)
    with pytest.raises(runner.RunnerError):
        runner.ephemeral_range()


def test_unreadable_range_fails_closed(monkeypatch):
    monkeypatch.setattr(runner.sys, "platform", "linux")
    def fail(_):
        raise OSError("unavailable")
    monkeypatch.setattr(Path, "read_text", fail)
    with pytest.raises(runner.RunnerError):
        runner.ephemeral_range()


def test_ephemeral_port_uses_kernel_range(monkeypatch):
    class Socket:
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def bind(self, address): assert address == ("127.0.0.1", 0)
        def getsockname(self): return ("127.0.0.1", 38153)
    monkeypatch.setattr(runner.socket, "socket", Socket)
    monkeypatch.setattr(runner, "ephemeral_range", lambda: (32768, 60999))
    assert runner.ephemeral_port() == 38153
    monkeypatch.setattr(runner, "ephemeral_range", lambda: (49152, 65535))
    with pytest.raises(runner.RunnerError):
        runner.ephemeral_port()
