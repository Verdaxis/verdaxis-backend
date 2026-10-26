"""The Phase 5 candidate generator changes one import or one generated block."""
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest
from scripts import rust_caddy_candidate as sites
from scripts import rust_phase5_candidate as generator

ROOT = Path(__file__).resolve().parents[2]
SNIPPET = ROOT / "deploy/caddy/rust-phase5.caddy"
PYTHON = "127.0.0.1:8001"
RUST = "127.0.0.1:8011"
PHASE1 = " ".join([PYTHON] * 6)


def staging_site(phase5=PYTHON):
    return (
        "api-staging.example {\n"
        "\troute {\n"
        f"\t\timport verdaxis_rust_public {PHASE1}\n"
        f"\t\timport verdaxis_rust_phase5 {phase5} {PYTHON}\n"
        "\t\thandle {\n"
        f"\t\t\treverse_proxy {PYTHON} {{\n"
        "\t\t\t\tflush_interval -1\n"
        "\t\t\t\ttransport http {\n"
        "\t\t\t\t\tkeepalive off\n"
        "\t\t\t\t}\n"
        "\t\t\t}\n"
        "\t\t}\n"
        "\t}\n"
        "}\n"
    )


def production_site():
    text = (ROOT / "tests/fixtures/caddy/production.Caddyfile").read_text()
    return sites.candidate(text, "production", "access-log")


def test_snippet_matchers_are_exact_and_share_one_switch():
    snippet = SNIPPET.read_text()
    definition = sites.blocks(snippet)[0]
    assert definition["header"] == "(verdaxis_rust_phase5)"
    matchers = [node for node in definition["children"] if node["header"].startswith("@rust_phase5_")]
    handles = [node for node in definition["children"] if node["header"].startswith("handle @rust_phase5_")]
    assert [node["header"] for node in matchers] == [
        "@rust_phase5_public", "@rust_phase5_compliance_get", "@rust_phase5_compliance_post",
    ]
    assert len(handles) == 3
    for node in matchers:
        body = snippet[node["start"]:node["end"]]
        # The rolled-back state (Rust argument equals Python's) stops matching.
        assert "expression '{args[0]}' != '{args[1]}'" in body
        patterns = re.findall(r"^\s*path_regexp (\S+)$", body, re.MULTILINE)
        assert len(patterns) == 1 and "path " not in body.replace("path_regexp", "")
        assert patterns[0].startswith("^/api/") and patterns[0].endswith("$")
        assert "*" not in patterns[0]
    for node in handles:
        body = snippet[node["start"]:node["end"]]
        # Rust first, the unchanged Python process second, and the same
        # forwarding, retry and passive-health options as the first read groups.
        assert "reverse_proxy {args[0]} {args[1]} {" in body
        for option in ("header_up X-Forwarded-For {client_ip}", "lb_policy first", "lb_try_duration 10s",
                       "lb_try_interval 250ms", "fail_duration 30s", "max_fails 1",
                       "flush_interval -1", "transport http {", "keepalive off"):
            assert option in body
    assert snippet.count("{args[") == 2 * 3 + 2 * 3
    assert "{args[2]}" not in snippet


def test_staging_candidate_moves_one_import_and_rolls_back_exactly():
    original = staging_site()
    moved = generator.candidate(original, "staging", "rust")
    assert moved.count(RUST) == 1
    assert moved.replace(RUST, PYTHON) == original
    assert generator.candidate(moved, "staging", "python") == original
    # The first read groups and the Python fallback are untouched.
    assert f"import verdaxis_rust_public {PHASE1}\n" in moved
    assert f"reverse_proxy {PYTHON} {{" in moved
    with pytest.raises(ValueError):
        generator.candidate(moved, "staging", "rust")
    with pytest.raises(ValueError):
        generator.candidate(original, "staging", "python")


@pytest.mark.parametrize("broken", [
    lambda text: text.replace(f"\t\timport verdaxis_rust_phase5 {PYTHON} {PYTHON}\n", ""),
    lambda text: text + text,
    lambda text: text.replace("\troute {", "\thandle {"),
    lambda text: text.replace(f"verdaxis_rust_phase5 {PYTHON} {PYTHON}", f"verdaxis_rust_phase5 {PYTHON}"),
    lambda text: text.replace(f"verdaxis_rust_phase5 {PYTHON} {PYTHON}", f"verdaxis_rust_phase5 {PYTHON} {RUST}"),
    lambda text: text.replace(f"verdaxis_rust_phase5 {PYTHON} {PYTHON}", f"verdaxis_rust_phase5 {PYTHON} {PYTHON} {PYTHON}"),
    lambda text: text.replace(f"verdaxis_rust_phase5 {PYTHON} {PYTHON}", f"verdaxis_rust_phase5 127.0.0.1:9 {PYTHON}"),
])
def test_staging_refuses_an_unreviewed_layout(broken):
    with pytest.raises(ValueError):
        generator.candidate(broken(staging_site()), "staging", "rust")


def test_environment_and_target_are_checked():
    with pytest.raises(ValueError):
        generator.candidate(staging_site(), "preview", "rust")
    with pytest.raises(ValueError):
        generator.candidate(staging_site(), "staging", "orderbook")


def test_production_inserts_one_block_between_analytics_and_python():
    logged = production_site()
    updated = generator.candidate(logged, "production", "rust")
    start = updated.index(generator.BEGIN)
    stop = updated.index(generator.END) + len(generator.END)
    block = updated[start:stop]
    assert updated[:start] + updated[stop:] == logged
    assert updated.index("handle /api/send") < start < updated.index("reverse_proxy localhost:8000")
    assert block.count("reverse_proxy 127.0.0.1:8010 127.0.0.1:8000 {") == 3
    assert "{args[" not in block
    for option in ("header_up X-Forwarded-For {client_ip}", "lb_policy first", "lb_try_duration 10s",
                   "lb_try_interval 250ms", "fail_duration 30s", "max_fails 1",
                   "flush_interval -1", "keepalive off"):
        assert option in block
    assert generator.candidate(updated, "production", "python") == logged


def test_production_composes_with_a_first_read_group_in_either_order():
    logged = production_site()
    with_group = sites.candidate(logged, "production", "simple")
    both = generator.candidate(with_group, "production", "rust")
    assert generator.candidate(both, "production", "python") == with_group
    other_order = sites.candidate(generator.candidate(logged, "production", "rust"), "production", "simple")
    assert generator.candidate(other_order, "production", "python") == with_group


def test_production_refuses_an_unreviewed_layout():
    logged = production_site()
    updated = generator.candidate(logged, "production", "rust")
    original = (ROOT / "tests/fixtures/caddy/production.Caddyfile").read_text()
    with pytest.raises(ValueError):
        generator.candidate(original, "production", "rust")  # no access log yet
    with pytest.raises(ValueError):
        generator.candidate(updated, "production", "rust")  # already present
    with pytest.raises(ValueError):
        generator.candidate(logged, "production", "python")  # nothing to remove
    with pytest.raises(ValueError):
        generator.candidate(updated.replace("max_fails 1", "max_fails 2", 1), "production", "python")
    with pytest.raises(ValueError):
        generator.candidate(logged.replace("localhost:8000", "localhost:9000"), "production", "rust")
    with pytest.raises(ValueError):
        generator.candidate(logged + logged, "production", "rust")
    imported = logged.replace("\t\thandle /script.js", f"\t\timport verdaxis_rust_phase5 {PYTHON} {PYTHON}\n\t\thandle /script.js")
    with pytest.raises(ValueError):
        generator.candidate(imported, "production", "rust")


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


def adapted(caddy, text, tmp_path, name):
    config = tmp_path / f"{name}.Caddyfile"
    # Relocate only the log file into the isolated account; never write /var/log.
    config.write_text(text.replace("/var/log/caddy/api.verdaxis.exchange.log", str(tmp_path / "access.log")))
    for verb in ("validate", "adapt"):
        result = subprocess.run([caddy, verb, "--config", str(config), "--adapter", "caddyfile"],
                                capture_output=True, text=True, timeout=30)
        print(f"{name} caddy {verb} EXIT={result.returncode}")
        assert result.returncode == 0, result.stderr
    return json.loads(result.stdout)


def test_real_caddy_adapts_every_candidate(tmp_path):
    caddy = shutil.which("caddy")
    if not caddy:
        pytest.skip("real Caddy is required for the test-host gate")
    snippet = SNIPPET.read_text() + (ROOT / "deploy/caddy/rust-public-staging.caddy").read_text()
    for name, text in (
        ("staging-python", staging_site()),
        ("staging-rust", generator.candidate(staging_site(), "staging", "rust")),
    ):
        document = adapted(caddy, snippet + text, tmp_path, name)
        phase5 = [p for p in proxies(document) if len(p["upstreams"]) == 2
                  and p["upstreams"][1]["dial"] == PYTHON
                  and p["upstreams"][0]["dial"] in {PYTHON, RUST} and "X-Forwarded-For" in json.dumps(p)]
        assert len(phase5) == 3, name
        for proxy in phase5:
            balancing = proxy["load_balancing"]
            assert balancing["selection_policy"] == {"policy": "first"}
            assert balancing["try_duration"] == 10_000_000_000
            assert proxy["health_checks"]["passive"] == {"fail_duration": 30_000_000_000, "max_fails": 1}
    current = production_site()
    for name, text in (("production-python", current),
                       ("production-rust", generator.candidate(current, "production", "rust"))):
        document = adapted(caddy, text, tmp_path, name)
        moved = [p for p in proxies(document) if any(u["dial"] == "127.0.0.1:8010" for u in p["upstreams"])]
        assert len(moved) == (3 if name.endswith("rust") else 0), name
        for proxy in moved:
            assert [u["dial"] for u in proxy["upstreams"]] == ["127.0.0.1:8010", "127.0.0.1:8000"]
            assert proxy["load_balancing"]["selection_policy"] == {"policy": "first"}


@pytest.mark.parametrize("environment,site,target", [
    ("staging", staging_site, "rust"),
    ("production", production_site, "rust"),
])
def test_cli_writes_complete_candidate_and_exact_private_rollback(tmp_path, environment, site, target):
    source = tmp_path / "Caddyfile"
    original = site().encode()
    source.write_bytes(original)
    output = tmp_path / "change"
    result = subprocess.run(
        [sys.executable, "-m", "scripts.rust_phase5_candidate", "--site", str(source),
         "--environment", environment, "--target", target, "--output-dir", str(output)],
        capture_output=True, text=True, timeout=30, cwd=ROOT)
    assert result.returncode == 0, result.stderr
    assert (output / "rollback.caddy").read_bytes() == original
    assert source.read_bytes() == original
    assert (output / "candidate.caddy").stat().st_mode & 0o777 == 0o600
    assert (output / "candidate.caddy").read_bytes() != original
