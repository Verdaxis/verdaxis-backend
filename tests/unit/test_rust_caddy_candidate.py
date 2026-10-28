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
    original = "site.example {\n route {\n  import verdaxis_rust_public " + " ".join([f"127.0.0.1:{python}"] * 6) + "\n  reverse_proxy 127.0.0.1:" + python + "\n }\n}\n"
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
    # The fallback is a required sixth argument and must be Python.
    python_token = f"127.0.0.1:{python}"
    six = " ".join([python_token] * 6)
    with pytest.raises(ValueError):
        module.candidate(original.replace(six, " ".join([python_token] * 5)), environment, "simple")
    with pytest.raises(ValueError):
        module.candidate(original.replace(six, " ".join([python_token] * 5 + [f"127.0.0.1:{rust}"])), environment, "simple")


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
        if group in module.READ_GROUPS:
            # Every read group tries its own upstream, then the shared Python
            # fallback, in that order, and remembers a refused connection.
            assert f"reverse_proxy {{args[{module.GROUPS.index(group)}]}} {{args[5]}} {{" in body
            assert "lb_policy first" in body
            assert "lb_try_duration 10s" in body
            assert "lb_try_interval 250ms" in body
            assert "fail_duration 30s" in body
            assert "max_fails 1" in body
        else:
            # MCP carries writes and must keep a single upstream.
            assert body.count("reverse_proxy ") == 1
            assert "{args[5]}" not in body
            assert "lb_try_duration" not in body
    contract = (ROOT / "tests/rust_contract/Caddyfile").read_text()
    assert "flush_interval -1" in contract and "keepalive off" in contract
    # The public import's last argument, the CRUD import's last argument and the
    # site fallback are the same Python upstream.
    assert contract.count("{$PYTHON_UPSTREAM}") == 3


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
    for option in ("reverse_proxy 127.0.0.1:8010 127.0.0.1:8000", "header_up X-Forwarded-For {client_ip}",
                   "lb_policy first", "lb_try_duration 10s", "lb_try_interval 250ms",
                   "fail_duration 30s", "max_fails 1", "flush_interval -1", "keepalive off"):
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
    assert "request>uri query {" in updated and "delete stream_token" in updated
    assert "roll_size 20MiB" in updated and "roll_keep 3" in updated
    with pytest.raises(ValueError):
        module.candidate(updated, "production", "access-log")
    for broken in (original.replace("api.verdaxis.exchange {", "other.example {"), original + original, original.replace("localhost:8000", "localhost:9000")):
        with pytest.raises(ValueError):
            module.candidate(broken, "production", "access-log")


def proxies(document):
    found = []

    def collect(node):
        if isinstance(node, dict):
            if node.get("handler") == "reverse_proxy":
                found.append(node)
            for value in node.values():
                collect(value)
        elif isinstance(node, list):
            for value in node:
                collect(value)

    collect(document)
    return found


def test_real_caddy_adapts_and_validates_production_candidates(tmp_path):
    import json
    import shutil
    import subprocess

    caddy = shutil.which("caddy")
    if not caddy:
        pytest.fail("real Caddy is required for the test-host gate")
    module = generator()
    current = production_fixture()
    for group in ["access-log", "fallback", "simple", "orderbook", "prices", "curves"]:
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
                adapted = proxies(document)
                catchalls = [p for p in adapted
                             if len(p["upstreams"]) == 1 and p["upstreams"][0]["dial"] == "localhost:8000"]
                assert len(catchalls) == 1
                if group == "access-log":
                    assert "load_balancing" not in catchalls[0]
                else:
                    assert catchalls[0]["load_balancing"]["try_duration"] == 15_000_000_000
                    assert catchalls[0]["load_balancing"]["try_interval"] == 250_000_000
                groups = [p for p in adapted
                          if any(u["dial"] == "127.0.0.1:8010" for u in p["upstreams"])]
                if group in module.READ_GROUPS:
                    assert len(groups) == module.READ_GROUPS.index(group) + 1
                    for proxy in groups:
                        assert [u["dial"] for u in proxy["upstreams"]] == [
                            "127.0.0.1:8010", "127.0.0.1:8000"]
                        balancing = proxy["load_balancing"]
                        assert balancing["selection_policy"] == {"policy": "first"}
                        assert balancing["try_duration"] == 10_000_000_000
                        assert balancing["try_interval"] == 250_000_000
                        assert proxy["health_checks"]["passive"] == {
                            "fail_duration": 30_000_000_000, "max_fails": 1}
                else:
                    assert groups == []
                encoder = document["logging"]["logs"]["log0"]["encoder"]
                assert encoder["format"] == "filter"
                assert encoder["wrap"]["format"] == "json"
                fields = dict(encoder["fields"])
                assert fields.pop("request>uri") == {"filter": "query", "actions": [{"type": "delete", "parameter": "stream_token"}]}
                assert all(value == {"filter": "delete"} for value in fields.values())


def test_real_caddy_gives_every_staging_group_the_python_fallback_transport(tmp_path):
    import json
    import shutil
    import subprocess

    caddy = shutil.which("caddy")
    if not caddy:
        pytest.fail("real Caddy is required for the test-host gate")
    module = generator()
    snippet = (ROOT / "deploy/caddy/rust-public-staging.caddy").read_text()
    prepared = snippet + """
http://:8080 {
\troute {
\t\timport verdaxis_rust_public 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001
\t\thandle {
\t\t\treverse_proxy 127.0.0.1:8001 {
\t\t\t\tflush_interval -1
\t\t\t\ttransport http {
\t\t\t\t\tkeepalive off
\t\t\t\t}
\t\t\t}
\t\t}
\t}
}
"""
    # Preparation sends all five groups to Python; the first cutover changes one.
    for name, text, rust_proxies in (
        ("prepared", prepared, 0),
        ("simple", module.candidate(prepared, "staging", "simple"), 1),
    ):
        config = tmp_path / f"{name}.Caddyfile"
        config.write_text(text)
        for verb in ("validate", "adapt"):
            command = [caddy, verb, "--config", str(config), "--adapter", "caddyfile"]
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
            print(f"{name} caddy {verb} EXIT={result.returncode}")
            assert result.returncode == 0, result.stderr
        result = subprocess.run([caddy, "adapt", "--config", str(config), "--adapter", "caddyfile"], capture_output=True, text=True, timeout=30)
        proxies = []

        def collect(node):
            if isinstance(node, dict):
                if node.get("handler") == "reverse_proxy":
                    proxies.append(node)
                for value in node.values():
                    collect(value)
            elif isinstance(node, list):
                for value in node:
                    collect(value)

        collect(json.loads(result.stdout))
        assert len(proxies) == 6, name
        for proxy in proxies:
            assert proxy["flush_interval"] == -1, name
            assert proxy["transport"]["protocol"] == "http", name
            assert proxy["transport"]["keep_alive"] == {"enabled": False}, name
        assert sum(proxy["upstreams"][0]["dial"] == "127.0.0.1:8011" for proxy in proxies) == rust_proxies
        # The four read groups carry Rust then Python; MCP and the final
        # catch-all keep a single upstream.
        read = [proxy for proxy in proxies if len(proxy["upstreams"]) == 2]
        single = [proxy for proxy in proxies if len(proxy["upstreams"]) == 1]
        assert len(read) == 4, name
        assert len(single) == 2, name
        for proxy in read:
            dials = [upstream["dial"] for upstream in proxy["upstreams"]]
            assert dials[1] == "127.0.0.1:8001", name
            assert dials[0] in {"127.0.0.1:8001", "127.0.0.1:8011"}, name
            balancing = proxy["load_balancing"]
            assert balancing["selection_policy"] == {"policy": "first"}, name
            assert balancing["try_duration"] == 10_000_000_000, name
            assert balancing["try_interval"] == 250_000_000, name
            assert proxy["health_checks"]["passive"] == {
                "fail_duration": 30_000_000_000, "max_fails": 1}, name
        for proxy in single:
            assert "load_balancing" not in proxy, name
            assert "health_checks" not in proxy, name


def staging_site():
    return (
        "api-staging.example {\n"
        "\troute {\n"
        "\t\timport verdaxis_rust_public 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 "
        "127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001\n"
        "\t\thandle {\n"
        "\t\t\treverse_proxy 127.0.0.1:8001 {\n"
        "\t\t\t\tflush_interval -1\n"
        "\t\t\t\ttransport http {\n"
        "\t\t\t\t\tkeepalive off\n"
        "\t\t\t\t}\n"
        "\t\t\t}\n"
        "\t\t}\n"
        "\t}\n"
        "}\n"
    )


def test_production_python_catchall_waits_across_a_restart():
    module = generator()
    logged = module.candidate(production_fixture(), "production", "access-log")
    updated = module.candidate(logged, "production", "fallback")
    proxy = "\t\t\treverse_proxy localhost:8000 {\n"
    retry = "\t\t\t\tlb_try_duration 15s\n\t\t\t\tlb_try_interval 250ms\n"
    assert updated.count("lb_try_duration") == 1
    assert updated.count("lb_try_interval") == 1
    assert updated.replace(proxy + retry, proxy, 1) == logged
    with pytest.raises(ValueError):
        module.candidate(updated, "production", "fallback")
    # The reviewed catch-all anchor is required and must stay unique.
    with pytest.raises(ValueError):
        module.candidate(logged.replace("reverse_proxy localhost:8000", "reverse_proxy localhost:9000"),
                          "production", "fallback")
    with pytest.raises(ValueError):
        module.candidate(logged.replace("\troute {", "\thandle {"), "production", "fallback")


def test_staging_python_catchall_waits_across_a_restart():
    module = generator()
    original = staging_site()
    updated = module.candidate(original, "staging", "fallback")
    proxy = "\t\t\treverse_proxy 127.0.0.1:8001 {\n"
    retry = "\t\t\t\tlb_try_duration 15s\n\t\t\t\tlb_try_interval 250ms\n"
    assert updated.count("lb_try_duration") == 1
    assert updated.replace(proxy + retry, proxy, 1) == original
    with pytest.raises(ValueError):
        module.candidate(updated, "staging", "fallback")
    # The import found first is not the final catch-all: insert a second handle.
    trailing = original.replace("\t\t}\n\t}\n}\n", "\t\t}\n\t\thandle /extra {\n\t\t\trespond 200\n\t\t}\n\t}\n}\n")
    with pytest.raises(ValueError):
        module.candidate(trailing, "staging", "fallback")
    with pytest.raises(ValueError):
        module.candidate(original.replace(" 127.0.0.1:8001\n", "\n", 1), "staging", "fallback")
    with pytest.raises(ValueError):
        module.candidate(original.replace(
            "127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001",
            "127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8011"),
            "staging", "fallback")


@pytest.mark.parametrize("group", ["access-log", "fallback"])
def test_cli_writes_complete_candidate_and_exact_private_rollback(tmp_path, group):
    import subprocess
    import sys

    source = tmp_path / "Caddyfile"
    original = production_fixture().encode()
    source.write_bytes(original)
    output = tmp_path / f"change-{group}"
    result = subprocess.run([sys.executable, str(ROOT / "scripts/rust_caddy_candidate.py"), "--site", str(source), "--environment", "production", "--group", group, "--output-dir", str(output)], capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert (output / "rollback.caddy").read_bytes() == original
    assert source.read_bytes() == original
    assert (output / "candidate.caddy").stat().st_mode & 0o777 == 0o600
    assert (output / "candidate.caddy").read_bytes() != original


def staging_crud_site():
    return (
        "api-staging.example {\n"
        "\troute {\n"
        "\t\timport verdaxis_rust_public 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 "
        "127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001\n"
        "\t\timport verdaxis_rust_crud 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 "
        "127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001\n"
        "\t\thandle {\n"
        "\t\t\treverse_proxy 127.0.0.1:8001 {\n"
        "\t\t\t\tflush_interval -1\n"
        "\t\t\t\ttransport http {\n"
        "\t\t\t\t\tkeepalive off\n"
        "\t\t\t\t}\n"
        "\t\t\t}\n"
        "\t\t}\n"
        "\t}\n"
        "}\n"
    )


def test_staging_crud_gate_then_one_group_never_both():
    module = generator()
    original = staging_crud_site()
    prepared = module.candidate(original, "staging", "preauth")
    assert prepared.count("127.0.0.1:8011") == 1
    assert prepared.replace("127.0.0.1:8011", "127.0.0.1:8001") == original
    with pytest.raises(ValueError):
        module.candidate(prepared, "staging", "preauth")
    crud_tokens = original.splitlines()[3].split()[2:]
    for index, group in enumerate(module.CRUD_GROUPS):
        updated = module.candidate(prepared, "staging", group)
        assert updated.count("127.0.0.1:8011") == 2
        expected = list(crud_tokens)
        expected[6] = "127.0.0.1:8011"
        expected[index] = "127.0.0.1:8011"
        assert updated.splitlines()[3].split()[2:] == expected
        with pytest.raises(ValueError):
            module.candidate(updated, "staging", group)
    # The public import is never touched by a CRUD move.
    moved = module.candidate(prepared, "staging", "feedback")
    assert moved.splitlines()[2] == prepared.splitlines()[2]
    # A public group move never touches the CRUD import either.
    simple = module.candidate(prepared, "staging", "simple")
    assert simple.splitlines()[3] == prepared.splitlines()[3]
    # A group cannot move before the authority, or without the reviewed import.
    with pytest.raises(ValueError):
        module.candidate(original, "staging", "feedback")
    with pytest.raises(ValueError):
        module.candidate(prepared.replace("import verdaxis_rust_crud", "import verdaxis_rust_other"),
                         "staging", "feedback")
    with pytest.raises(ValueError):
        module.candidate(prepared.replace("127.0.0.1:8011", "127.0.0.1:8001", 1),
                         "staging", "feedback")
    broken = prepared.replace("import verdaxis_rust_crud", "handle verdaxis_rust_crud")
    with pytest.raises(ValueError):
        module.candidate(broken, "staging", "feedback")


def test_production_crud_gate_then_one_group_and_exact_rollback():
    module = generator()
    logged = module.candidate(production_fixture(), "production", "access-log")
    with pytest.raises(ValueError):
        module.candidate(logged, "production", "feedback")
    with pytest.raises(ValueError):
        module.candidate(production_fixture(), "production", "preauth")
    prepared = module.candidate(logged, "production", "preauth")
    assert prepared.count(module.CRUD_BEGIN) == 1
    assert prepared.count("reverse_proxy @admin_pre_auth 127.0.0.1:8010 127.0.0.1:8000") == 1
    assert prepared.count("reverse_proxy 127.0.0.1:8000 127.0.0.1:8000 {") == 15
    assert prepared.index(module.CRUD_BEGIN) < prepared.index("\t\thandle {\n\t\t\treverse_proxy localhost:8000")
    with pytest.raises(ValueError):
        module.candidate(prepared, "production", "preauth")
    for group in module.CRUD_GROUPS:
        updated = module.candidate(prepared, "production", group)
        count = module.CRUD_HANDLE_COUNTS[group]
        assert updated.count("reverse_proxy 127.0.0.1:8010 127.0.0.1:8000 {") == count
        assert module.rollback(updated, "production", group) == prepared
        with pytest.raises(ValueError):
            module.rollback(updated, "production", "preauth")
        with pytest.raises(ValueError):
            module.candidate(updated, "production", group)
    assert module.rollback(prepared, "production", "preauth") == logged
    # An unknown or missing group is refused rather than editing a proxy.
    with pytest.raises(ValueError):
        module.rollback(prepared, "production", "mcp")


def test_gate_rollback_accepts_older_rendering_with_python_owners():
    module = generator()
    logged = module.candidate(production_fixture(), "production", "access-log")
    prepared = module.candidate(logged, "production", "preauth")
    older = prepared.replace("lb_try_duration 10s", "lb_try_duration 9s")
    assert module.rollback(older, "production", "preauth") == logged


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_real_caddy_adapts_crud_candidates(tmp_path, environment):
    import json
    import shutil
    import subprocess

    caddy = shutil.which("caddy")
    if not caddy:
        pytest.fail("real Caddy is required for the test-host gate")
    module = generator()
    if environment == "staging":
        snippet = (ROOT / "deploy/caddy/rust-public-staging.caddy").read_text()
        current = snippet + (ROOT / "deploy/caddy/rust-crud.caddy").read_text() + """
http://:8080 {
\troute {
\t\timport verdaxis_rust_public 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001
\t\timport verdaxis_rust_crud 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001 127.0.0.1:8001
\t\thandle {
\t\t\treverse_proxy 127.0.0.1:8001 {
\t\t\t}
\t\t}
\t}
}
"""
        steps = ["preauth", "feedback", "alerts"]
    else:
        current = module.candidate(production_fixture(), "production", "access-log")
        steps = ["preauth", "feedback", "referrals"]
    for step in steps:
        current = module.candidate(current, environment, step)
    config = tmp_path / f"{environment}.Caddyfile"
    config.write_text(current.replace(
        "/var/log/caddy/api.verdaxis.exchange.log", str(tmp_path / "access.log")))
    for verb in ("validate", "adapt"):
        command = [caddy, verb, "--config", str(config), "--adapter", "caddyfile"]
        result = subprocess.run(command, capture_output=True, text=True, timeout=30)
        print(f"{environment} {verb} EXIT={result.returncode}")
        assert result.returncode == 0, result.stderr
    document = json.loads(subprocess.run(
        [caddy, "adapt", "--config", str(config), "--adapter", "caddyfile"],
        capture_output=True, text=True, timeout=30, check=True).stdout)
    expressed = json.dumps(document)
    assert "/internal/admin-preauth" in expressed
    assert "X-Verdaxis-Preauth-Counted" in expressed
    adapted_proxies = proxies(document)
    crud_proxies = [proxy for proxy in adapted_proxies
                    if proxy.get("load_balancing", {}).get("try_duration") == 10_000_000_000]
    # Staging also imports the four public read pools before the CRUD pools.
    assert len(crud_proxies) == (19 if environment == "staging" else 15)
    for proxy in crud_proxies[-15:]:
        assert proxy["headers"]["request"]["set"]["X-Forwarded-For"] == ["{http.vars.client_ip}"]
    gate = [proxy for proxy in adapted_proxies if proxy.get("rewrite", {}).get("uri") == "/internal/admin-preauth"]
    assert len(gate) == 1
    assert gate[0]["rewrite"]["method"] == "GET"
    assert len(gate[0]["handle_response"]) == 3
    assert [upstream["dial"] for upstream in gate[0]["upstreams"]] == [
        "127.0.0.1:8010" if environment == "production" else "127.0.0.1:8011",
        "127.0.0.1:8000" if environment == "production" else "127.0.0.1:8001"]
    rust = "127.0.0.1:8010" if environment == "production" else "127.0.0.1:8011"
    assert len([proxy for proxy in adapted_proxies
                if any(upstream["dial"] == rust for upstream in proxy["upstreams"])]) == (
        1 + sum(module.CRUD_HANDLE_COUNTS[step] for step in steps[1:]))
