"""Offline all-groups site contracts; fixtures are not installed inventory."""
from __future__ import annotations

import hashlib
import importlib
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def module():
    return importlib.import_module("scripts.rust_phase10_window_caddy")


def saved_site(environment):
    m = module()
    if environment == "production":
        old = (ROOT / "tests/fixtures/caddy/production.Caddyfile").read_text()
        helpers = importlib.import_module("scripts.rust_caddy_candidate")
        old = helpers.candidate(old, environment, "access-log")
    else:
        old = "api-staging.verdaxis.exchange {\n\troute {\n\t\thandle {\n\t\t\treverse_proxy localhost:8001 {\n\t\t\t\theader_up X-Forwarded-For {client_ip}\n\t\t\t\tflush_interval -1\n\t\t\t\ttransport http {\n\t\t\t\t\tkeepalive off\n\t\t\t\t}\n\t\t\t}\n\t\t}\n\t}\n}\n"
    python, rust = m.UPSTREAMS[environment]
    live = "".join(m.snippet_body(name, [rust, python]) for name in ("rust-abilities.caddy", "rust-offboarding.caddy"))
    at = old.rfind("\n\t\thandle {\n") + 1
    return old[:at] + live + old[at:]


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_complete_site_keeps_live_groups_and_removes_python_dials(environment):
    m = module()
    original = saved_site(environment)
    updated = m.candidate(original, environment)
    python, rust = m.UPSTREAMS[environment]
    for line in updated.splitlines():
        if line.strip().startswith("reverse_proxy"):
            assert python not in line and "localhost:" + python.rsplit(":", 1)[1] not in line
    for name in ("rust-abilities.caddy", "rust-offboarding.caddy"):
        assert m.snippet_body(name, [rust, python]) in updated
    assert "reverse_proxy @admin_pre_auth " + rust in updated
    for group in ("notifications", "preferences", "feedback", "watchlists", "alerts", "referrals"):
        assert "handle @rust_crud_" + group in updated
    assert "@credential_area" in updated and "@phase4_0" in updated
    assert "@root_health" in updated and "@monitor_canary" in updated
    assert "@phase7_rfq_create" in updated
    assert "{args[" not in updated and "\n\t\timport " not in updated
    assert "header_up X-Forwarded-For {client_ip}" in updated
    assert "flush_interval -1" in updated and "keepalive off" in updated
    if environment == "production":
        assert 'unrelated.example {\n\trespond "unchanged"\n}' in updated
        assert "reverse_proxy localhost:8700" in updated
        assert updated.index("handle /api/send") < updated.index("@credential_area")
    probes = m.probes(environment)
    assert {p["group"] for p in probes} == set(m.PROBE_ROUTES)
    assert all(p["method"] and p["path"].startswith("/") for p in probes)


def test_staging_saved_imports_are_resolved_without_reading_live_paths():
    m = module()
    original = saved_site("staging")
    python, rust = m.UPSTREAMS["staging"]
    for name, symbol in (("rust-abilities.caddy", "verdaxis_rust_abilities"), ("rust-offboarding.caddy", "verdaxis_rust_offboarding")):
        original = original.replace(m.snippet_body(name, [rust, python]), f"\t\timport {symbol} {rust} {python}\n")
    assert "import " not in m.candidate(original, "staging")
    with pytest.raises(ValueError, match="import"):
        m.candidate(original.replace("\troute {", "\troute {\n import /etc/caddy/unknown"), "staging")


@pytest.mark.parametrize("change", [lambda s: s.replace("@rust_abilities", "@other_abilities"), lambda s: s.replace("localhost:8000", "10.0.0.1:8000"), lambda s: s.replace("\troute {", "\thandle {"), lambda s: s.replace("\troute {", "\troute {\n respond 200")])
def test_unreviewed_site_is_refused(change):
    m = module()
    original = saved_site("production")
    changed = change(original)
    if changed != original:
        with pytest.raises(ValueError):
            m.candidate(changed, "production")


def test_private_bundle_exact_rollback_and_source_unchanged(tmp_path, monkeypatch):
    m = module()
    source = tmp_path / "saved.caddy"
    original = saved_site("production").replace("\n", "\r\n").encode()
    source.write_bytes(original)
    calls = []
    def validate(config, binary, digest):
        calls.append(config.read_bytes())
        return {"binary_sha256": digest, "validate": "passed", "adapt": "passed"}
    monkeypatch.setattr(m, "validate", validate)
    output = tmp_path / "private"
    m.create_bundle(source, "production", output, Path("/trusted/caddy"), "a" * 64)
    assert source.read_bytes() == original
    assert (output / "rollback.caddy").read_bytes() == original
    assert len(calls) == 1
    assert output.stat().st_mode & 0o777 == 0o700
    for p in output.iterdir():
        assert p.stat().st_mode & 0o777 == 0o600
    manifest = json.loads((output / "manifest.json").read_text())
    assert manifest["rollback_sha256"] == hashlib.sha256(original).hexdigest()
    assert manifest["validation"]["validate"] == "passed"
    assert manifest["files"] == ["candidate.caddy", "rollback.caddy", "probes.json"]
    with pytest.raises((FileExistsError, ValueError)):
        m.create_bundle(source, "production", output, Path("/trusted/caddy"), "a" * 64)


def test_validator_requires_absolute_digest_bound_executable(tmp_path):
    m = module()
    with pytest.raises(ValueError):
        m.validate(tmp_path / "candidate.caddy", Path("caddy"), "a" * 64)
    fake = tmp_path / "caddy"
    fake.write_text("#!/bin/sh\nexit 0\n")
    fake.chmod(0o700)
    with pytest.raises(ValueError, match="digest"):
        m.validate(tmp_path / "candidate.caddy", fake, "b" * 64)


@pytest.mark.parametrize("environment", ["staging", "production"])
def test_real_caddy_validate_and_adapt(environment, tmp_path):
    binary = os.environ.get("PHASE10_CADDY_BIN") or shutil.which("caddy")
    if not binary:
        pytest.skip("trusted local Caddy executable unavailable")
    m = module()
    config = tmp_path / "candidate.caddy"
    config.write_text(m.candidate(saved_site(environment), environment))
    digest = hashlib.sha256(Path(binary).read_bytes()).hexdigest()
    result = m.validate(config, Path(binary), digest)
    assert result["validate"] == result["adapt"] == "passed"
    adapted = json.loads(subprocess.check_output([binary, "adapt", "--config", str(config), "--adapter", "caddyfile"], stderr=subprocess.DEVNULL))
    def walk(value):
        if isinstance(value, dict):
            yield value
            for child in value.values():
                yield from walk(child)
        elif isinstance(value, list):
            for child in value:
                yield from walk(child)
    dials = [node["dial"] for node in walk(adapted) if "dial" in node]
    python, rust = m.UPSTREAMS[environment]
    assert rust in dials
    assert python not in dials and "localhost:" + python.rsplit(":", 1)[1] not in dials


def test_failed_validation_cannot_publish_acceptance_manifest(tmp_path, monkeypatch):
    m = module()
    source = tmp_path / "saved.caddy"
    source.write_text(saved_site("staging"))
    def refuse(*args):
        raise ValueError("validation failed")
    monkeypatch.setattr(m, "validate", refuse)
    output = tmp_path / "private"
    with pytest.raises(ValueError, match="validation failed"):
        m.create_bundle(source, "staging", output, Path("/trusted/caddy"), "a" * 64)
    assert not (output / "manifest.json").exists()
    assert (output / "rollback.caddy").read_bytes() == source.read_bytes()
    assert not list(output.glob(".caddy-check-*"))


def test_existing_public_and_crud_groups_can_be_composed():
    m = module()
    helpers = importlib.import_module("scripts.rust_caddy_candidate")
    original = saved_site("production")
    for group in ("simple", "prices", "preauth", "feedback", "alerts"):
        original = helpers.candidate(original, "production", group)
    assert "@rust_crud_feedback" in m.candidate(original, "production")
