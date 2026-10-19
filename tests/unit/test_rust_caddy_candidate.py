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


@pytest.mark.parametrize("environment,python,rust", [("staging", "8001", "8011")])
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


def test_all_snippet_groups_preserve_staging_forwarding_options():
    module = generator()
    snippet = (ROOT / "deploy/caddy/rust-public-staging.caddy").read_text()
    definition = module.blocks(snippet)[0]
    for group in module.GROUPS:
        handle = module.one(definition["children"], f"handle @rust_{group}")
        body = snippet[handle["start"]:handle["end"]]
        assert "flush_interval -1" in body
        assert "transport http {" in body
        assert "keepalive off" in body
    contract = (ROOT / "tests/rust_contract/Caddyfile").read_text()
    assert "flush_interval -1" in contract and "keepalive off" in contract


def production_fixture():
    return (ROOT / "tests/fixtures/caddy/production.Caddyfile").read_text()


@pytest.mark.parametrize("group", ["simple", "orderbook", "prices", "curves"])
def test_production_inserts_one_group_between_analytics_and_python(group):
    module = generator()
    original = production_fixture()
    logged = module.candidate(original, "production", "access-log")
    updated = module.candidate(logged, "production", group)
    begin = f"\t\t# BEGIN verdaxis-rust {group}\n"
    end = f"\t\t# END verdaxis-rust {group}\n"
    start, stop = updated.index(begin), updated.index(end) + len(end)
    block = updated[start:stop]
    assert updated[:start] + updated[stop:] == logged
    assert updated.index("handle /api/send") < start < updated.index("reverse_proxy localhost:8000")
    for option in ("reverse_proxy 127.0.0.1:8010", "header_up X-Forwarded-For {client_ip}", "flush_interval -1", "keepalive off"):
        assert option in block
    assert module.rollback(updated, "production", group) == logged
    with pytest.raises(ValueError):
        module.candidate(updated, "production", group)
    with pytest.raises(ValueError):
        module.candidate(logged, "production", "mcp")
    with pytest.raises(ValueError):
        module.candidate(original, "production", group)
    earlier = module.candidate(logged, "production", "simple")
    if group != "simple":
        combined = module.candidate(earlier, "production", group)
        assert module.rollback(combined, "production", group) == earlier


def test_production_access_log_is_a_separate_redacted_change():
    module = generator()
    original = production_fixture()
    updated = module.candidate(original, "production", "access-log")
    assert "# Temporary log block removed after diagnosis." not in updated
    assert "reverse_proxy 127.0.0.1:8010" not in updated
    assert updated.split("\troute {", 1)[1] == original.split("\troute {", 1)[1]
    assert "output file /var/log/caddy/api.verdaxis.exchange.log" in updated
    for field in ("request>headers>Authorization", "request>headers>Cookie", "request>headers>X-Monitor-Token", "resp_headers>Set-Cookie"):
        assert f"{field} delete" in updated
    assert "roll_size 20MiB" in updated and "roll_keep 3" in updated
    with pytest.raises(ValueError):
        module.candidate(updated, "production", "access-log")
    for broken in (original.replace("api.verdaxis.exchange {", "other.example {"), original + original, original.replace("localhost:8000", "localhost:9000")):
        with pytest.raises(ValueError):
            module.candidate(broken, "production", "access-log")


def test_real_caddy_adapts_and_validates_production_candidates(tmp_path):
    import json
    import shutil
    import subprocess

    caddy = shutil.which("caddy")
    if not caddy:
        pytest.skip("real Caddy is required for the test-host gate")
    module = generator()
    current = module.candidate(production_fixture(), "production", "access-log")
    for group in ["access-log", "simple", "orderbook", "prices", "curves"]:
        if group != "access-log":
            current = module.candidate(current, "production", group)
        # Relocate only the log file into the isolated account; never write /var/log.
        config = tmp_path / f"{group}.Caddyfile"
        config.write_text(current.replace("/var/log/caddy/api.verdaxis.exchange.log", str(tmp_path / "access.log")))
        for command in ([caddy, "validate", "--config", str(config), "--adapter", "caddyfile"], [caddy, "adapt", "--config", str(config), "--adapter", "caddyfile"]):
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
            print(f"{group} {command[1]} EXIT={result.returncode}")
            assert result.returncode == 0, result.stderr
            if command[1] == "adapt":
                document = json.loads(result.stdout)
                text = json.dumps(document)
                for field in ("request>headers>Authorization", "request>headers>Cookie", "request>headers>X-Monitor-Token", "resp_headers>Set-Cookie"):
                    assert field in text
                if group != "access-log":
                    assert "127.0.0.1:8010" in text
                encoder = document["logging"]["logs"]["log0"]["encoder"]
                assert encoder["format"] == "filter"
                assert encoder["wrap"]["format"] == "json"
                assert all(value == {"filter": "delete"} for value in encoder["fields"].values())


def test_cli_writes_complete_candidate_and_exact_private_rollback(tmp_path):
    import subprocess
    import sys

    source = tmp_path / "Caddyfile"
    original = production_fixture().encode()
    source.write_bytes(original)
    output = tmp_path / "change"
    result = subprocess.run([sys.executable, str(ROOT / "scripts/rust_caddy_candidate.py"), "--site", str(source), "--environment", "production", "--group", "access-log", "--output-dir", str(output)], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert (output / "rollback.caddy").read_bytes() == original
    assert source.read_bytes() == original
    assert (output / "candidate.caddy").stat().st_mode & 0o777 == 0o600
