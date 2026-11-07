"""Production authority and bootstrap, without contacting deployed services."""
from __future__ import annotations

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_production_policy_is_the_reviewed_activation_with_the_bootstrap_unit():
    policy = json.loads((ROOT / "deploy/rust/production-policy.json").read_text())
    assert policy == {
        "version": 1,
        "environment": "production",
        "rust_enabled": True,
        "service_name": "verdaxis-rust.service",
        "bind": "127.0.0.1:8010",
        "health_url": "http://127.0.0.1:8010/health/ready",
        "db_session_time_zone": "UTC",
        "staged_releases_dir": ".runtime-rust/releases",
    }
    unit = (ROOT / "deploy/systemd/verdaxis-rust.service").read_text()
    manifest = (ROOT / "deploy/systemd/runtime-units.manifest").read_text().splitlines()
    assert "production\tverdaxis-rust.service" in manifest
    base = "/home/verdaxis-prod/verdaxis/prod/be"
    for expected in (
        f"WorkingDirectory={base}\n",
        f"EnvironmentFile={base}/.runtime-rust/runtime.env\n",
        "--environment production --expected-release-sha ${RELEASE_SHA}",
        "--bind 127.0.0.1:8010",
        "MemoryHigh=640M\n", "MemoryMax=768M\n",
        "CPUQuota=100%\n", "CPUWeight=50\n",
    ):
        assert expected in unit
    assert f"EnvironmentFile={base}/.env\n" not in unit
    assert "/staging/" not in unit
    assert "verdaxis-read guard rust-start --expected-release-sha ${RELEASE_SHA}" in unit
    assert "--revision-file" in unit
    assert "python" not in unit
    assert "--environment-file" not in unit
    assert "venv/bin/python" not in unit


def test_prod_publication_is_a_push_gate_not_a_pr_artifact():
    workflow = (ROOT / ".github/workflows/rust-read-ci.yml").read_text()
    assert "branches: [staging, prod]" in workflow
    assert "github.event_name == 'push'" in workflow
    import yaml

    jobs = yaml.safe_load(workflow)["jobs"]
    publish = jobs["publish"]
    assert set(publish["needs"]) == {"release", "contract"}
    assert "needs.release.result == 'success'" in publish["if"]
    assert "needs.contract.result == 'success'" in publish["if"]
    aggregate = jobs["contract"]
    assert "offline" in aggregate["needs"]
    assert aggregate["if"] == "always()"
    check = next(step for step in aggregate["steps"] if step.get("name") ==
                 "Require every native and tooling check to succeed")
    assert check["env"]["CHECK_RESULTS"] == "${{ toJSON(needs) }}"
    assert 'result["result"] != "success"' in check["run"]
    assert "assert not failed" in check["run"]
