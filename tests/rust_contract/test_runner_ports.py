"""The disposable allocator must not depend on the kernel's ephemeral range."""

from unittest.mock import MagicMock

from scripts.rust_contract import run_disposable


def test_allocator_binds_an_allowed_port_and_skips_occupied_ports(monkeypatch):
    probe = MagicMock()
    probe.__enter__.return_value = probe
    probe.getsockname.return_value = ("127.0.0.1", 38653)
    probe.bind.side_effect = [OSError("occupied"), None]
    monkeypatch.setattr(run_disposable.socket, "socket", lambda: probe)
    monkeypatch.setattr(run_disposable.secrets, "randbelow", lambda _: 0)

    port = run_disposable.ephemeral_port()

    assert port == 49153
    assert probe.bind.call_args_list[0].args == (("127.0.0.1", 49152),)
    assert probe.bind.call_args_list[1].args == (("127.0.0.1", 49153),)
