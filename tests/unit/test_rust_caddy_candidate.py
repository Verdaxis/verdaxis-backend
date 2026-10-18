"""Offline Caddy candidates preserve every byte outside one upstream."""
import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def generator():
    spec = importlib.util.spec_from_file_location("rust_caddy_candidate", ROOT / "scripts/rust_caddy_candidate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("environment,python,rust", [("production", "8000", "8010"), ("staging", "8001", "8011")])
def test_candidate_changes_one_group_and_keeps_rollback_exact(environment, python, rust):
    module = generator()
    original = "site.example {\n route {\n  import verdaxis_rust_public " + " ".join([f"127.0.0.1:{python}"] * 5) + "\n  reverse_proxy 127.0.0.1:" + python + "\n }\n}\n"
    for index, group in enumerate(["simple", "orderbook", "prices", "curves"]):
        candidate = module.candidate(original, environment, group)
        assert candidate.count(f"127.0.0.1:{rust}") == 1
        assert candidate.replace(f"127.0.0.1:{rust}", f"127.0.0.1:{python}") == original
        with pytest.raises(ValueError):
            module.candidate(candidate, environment, group)
    with pytest.raises(ValueError):
        module.candidate(original + original, environment, "simple")
    with pytest.raises(ValueError):
        module.candidate(original.replace(" route {", " handle {"), environment, "simple")
    if environment == "production":
        with pytest.raises(ValueError):
            module.candidate(original, environment, "mcp")
