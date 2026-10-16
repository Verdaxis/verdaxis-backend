"""The runner uses the same kernel range as disposable target guards."""

from unittest.mock import MagicMock

import pytest

from scripts.rust_contract import run_disposable


@pytest.mark.parametrize("port,allowed", [(38653, True), (61000, False)])
def test_allocator_checks_shared_kernel_range(monkeypatch, port, allowed):
    probe = MagicMock()
    probe.__enter__.return_value = probe
    probe.getsockname.return_value = ("127.0.0.1", port)
    monkeypatch.setattr(run_disposable.socket, "socket", lambda: probe)
    monkeypatch.setattr(run_disposable, "ephemeral_range", lambda: (32768, 60999))
    if allowed:
        assert run_disposable.ephemeral_port() == port
    else:
        with pytest.raises(run_disposable.RunnerError):
            run_disposable.ephemeral_port()
    probe.bind.assert_called_once_with(("127.0.0.1", 0))
