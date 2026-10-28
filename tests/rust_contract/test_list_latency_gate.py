"""Validate the measured p95 gate without requiring a database in unit tests."""

import pytest

from scripts.rust_contract.measure_lists import require_latency_parity


def test_latency_gate_rejects_slower_rust():
    rows = [
        dict(path="/api/orderbook", mode="warm", target="python", n=30, p95=1.8),
        dict(path="/api/orderbook", mode="warm", target="rust", n=30, p95=8.5),
    ]
    with pytest.raises(AssertionError, match="slower"):
        require_latency_parity(rows)


def test_latency_gate_requires_complete_measurements():
    with pytest.raises(AssertionError, match="complete"):
        require_latency_parity([])
