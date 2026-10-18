"""Production authority and bootstrap, without contacting deployed services."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from tests.unit.test_rust_release import RustReleaseFixture, _git, _load_preparer

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40


def test_production_policy_and_unit_are_a_policy_off_bootstrap():
    policy = json.loads((ROOT / "deploy/rust/production-policy.json").read_text())
    assert policy == {
        "version": 1,
        "environment": "production",
        "rust_enabled": False,
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
        "MemoryHigh=192M\n", "MemoryMax=256M\n",
        "CPUQuota=100%\n", "CPUWeight=50\n",
    ):
        assert expected in unit
    assert f"EnvironmentFile={base}/.env\n" not in unit
    assert "/staging/" not in unit
    assert "verify_migration_revision.py --expected ${MIGRATION_REVISION}" in unit
    assert "prepare_rust_budget.py --verify" in unit


def test_production_runtime_projects_only_its_app_identity(tmp_path):
    preparer = _load_preparer()
    env = tmp_path / ".env"
    env.write_text(
        f"ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod\n"
        "KYC_EXECUTION_GATE_ENABLED=false\nJWT_SECRET=do-not-project\n"
    )
    result = preparer.prepare(
        env, SHA, tmp_path / "runtime", ROOT, "p0_20261016_union",
        environment="production",
    )
    text = result["runtime_env"].read_text()
    assert "ENVIRONMENT=production\n" in text
    assert "verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod" in text
    assert "JWT_SECRET" not in text
    assert "KYC_EXECUTION_GATE_ENABLED=false\n" in text
    env.write_text(env.read_text().replace("verdaxis_app:", "verdaxis_app_staging:"))
    with pytest.raises(preparer.RustRuntimeError, match="identity"):
        preparer.prepare(
            env, SHA, tmp_path / "runtime", ROOT, "p0_20261016_union",
            environment="production",
        )


class ProductionReleaseFixture(RustReleaseFixture):
    """Reuse the real transaction fixture, with separate production source."""

    def __init__(self, tmp_path):
        super().__init__(tmp_path)
        _git(self.source, "branch", "-m", "prod")
        _git(self.source, "push", "-q", "origin", "prod")
        _git(self.checkout, "fetch", "-q", "origin")
        _git(self.checkout, "checkout", "-q", "-b", "prod", "origin/prod")

    def _write_source_tree(self):
        super()._write_source_tree()
        for name in ("production-policy.json",):
            (self.source / "deploy/rust" / name).write_bytes(
                (ROOT / "deploy/rust" / name).read_bytes()
            )
        manifest = self.source / "deploy/systemd/runtime-units.manifest"
        manifest.write_text(manifest.read_text() + "production\tverdaxis-backend.service\nproduction\tverdaxis-rust.service\n")
        for unit in ("verdaxis-backend.service", "verdaxis-rust.service"):
            (self.source / "deploy/systemd" / unit).write_text(
                "[Unit]\nDescription=production fixture\n[Service]\nExecStart=/bin/true\n"
            )
        (self.source / "deploy/migration-checkpoints.tsv").write_text(
            "p0_20261016_union\tp0_20261016_union\n"
        )

    def push_rust_release(self, *, enabled):
        policy_path = self.source / "deploy/rust/production-policy.json"
        policy = json.loads(policy_path.read_text())
        policy["rust_enabled"] = enabled
        policy_path.write_text(json.dumps(policy))
        (self.source / "release.txt").write_text(str(enabled))
        _git(self.source, "add", "-A")
        _git(self.source, "commit", "-qm", "production release")
        _git(self.source, "push", "-q", "origin", "prod")
        return _git(self.source, "rev-parse", "HEAD").stdout.strip()

    def deploy_environment(self, release_sha, *, fail_phase=""):
        env = super().deploy_environment(release_sha, fail_phase=fail_phase)
        env.update(
            TARGET_BRANCH="prod", DEPLOY_ENVIRONMENT="production",
            SERVICE_NAME="verdaxis-backend.service",
            MIGRATION_EXPECTED_CURRENT_REVISION="p0_20261016_union",
            MIGRATION_TARGET_REVISION="p0_20261016_union",
            DEPLOY_CURL_BODY=json.dumps({"status": "ok", "db": "ok", "environment": "production", "release_sha": release_sha}),
        )
        return env

    def deployed_env_text(self, release_sha, *, kyc_gate="false"):
        return (super().deployed_env_text(release_sha, kyc_gate=kyc_gate)
                .replace("ENVIRONMENT=staging", "ENVIRONMENT=production")
                .replace("verdaxis_staging", "verdaxis_prod")
                .replace("verdaxis_app_staging", "verdaxis_app")
                .replace("verdaxis_migrator_staging", "verdaxis_migrator"))

    def _write_fake_tools(self):
        super()._write_fake_tools()
        systemctl = self.fake_bin / "systemctl"
        systemctl.write_text(systemctl.read_text().replace(
            "verdaxis-rust-staging.service", "verdaxis-rust.service"
        ))

    def sign_for_prod(self):
        record = self.artifact_root / "provenance-record.json"
        body = json.loads(record.read_text())
        body["workflow_ref"] = "refs/heads/prod"
        data = json.dumps(body).encode()
        record.write_bytes(data)
        (self.artifact_root / "provenance-record.sig").write_bytes(self.key.sign(data))


@pytest.mark.parametrize("failure", ["", "unit", "provenance", "readiness"])
def test_production_activation_requires_its_unit_provenance_and_readiness(tmp_path, failure):
    fixture = ProductionReleaseFixture(tmp_path)
    sha = fixture.push_rust_release(enabled=True)
    fixture.build_artifact(sha)
    if failure != "provenance":
        fixture.sign_for_prod()
    overrides = {}
    if failure == "unit":
        overrides["DEPLOY_RUST_LOAD_STATE"] = "not-found"
    if failure == "readiness":
        overrides["DEPLOY_RUST_UNREADY"] = "1"
    result = fixture.deploy(sha, **overrides)
    if not failure:
        assert result.returncode == 0, result.stdout + result.stderr
        assert any("restart verdaxis-backend.service verdaxis-rust.service" in x for x in fixture.ledger)
        assert any("127.0.0.1:8010/health/ready" in x for x in fixture.ledger)
        assert not (tmp_path / "runtime-deploy-state/production.state").exists()
    else:
        assert result.returncode != 0, result.stdout + result.stderr
        if failure in {"unit", "provenance"}:
            assert _git(fixture.checkout, "rev-parse", "HEAD").stdout.strip() == fixture.old_sha
            assert not any("systemctl restart" in x for x in fixture.ledger)
        else:
            assert "DEPLOYMENT_STATE=blocked" in (tmp_path / "runtime-deploy-state/production.state").read_text()
            assert "verdaxis-rust.service" in fixture.stopped_services()


def test_production_policy_off_preserves_python_only_deploy(tmp_path):
    fixture = ProductionReleaseFixture(tmp_path)
    sha = fixture.push_rust_release(enabled=False)
    result = fixture.deploy(sha)
    assert result.returncode == 0, result.stdout + result.stderr
    assert not any("verify_rust_artifact.py" in x or "prepare_rust_runtime.py" in x for x in fixture.ledger)


def test_prod_publication_is_a_push_gate_not_a_pr_artifact():
    workflow = (ROOT / ".github/workflows/rust-read-ci.yml").read_text()
    assert "branches: [staging, prod]" in workflow
    assert "github.event_name == 'push'" in workflow
    assert "needs.offline.result == 'success'" in workflow
    assert "needs.contract.result == 'success'" in workflow
