"""Production authority and bootstrap, without contacting deployed services."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from tests.unit.test_rust_release import RustReleaseFixture, _git, _load_preparer

ROOT = Path(__file__).resolve().parents[2]
SHA = "a" * 40


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
        "MemoryHigh=192M\n", "MemoryMax=256M\n",
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


@pytest.mark.parametrize("revision", ["sqlx_1"])
def test_production_runtime_projects_its_app_identity_and_phase2(tmp_path, revision):
    preparer = _load_preparer()
    env = tmp_path / ".env"
    env.write_text(
        f"ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod\n"
        "KYC_EXECUTION_GATE_ENABLED=false\n"
        "JWT_SECRET=" + "k" * 40 + "\n"
        "JWT_ISSUER=verdaxis-prod-api\n"
        "JWT_AUDIENCE=verdaxis-prod-web\n"
        "FRONTEND_URL=https://app.verdaxis.exchange\n"
        "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\n"
        "RESEND_API_KEY=re_fixture_key\n"
        "OPENAI_API_KEY=sk-unrelated\n"
    )
    result = preparer.prepare(
        env, SHA, tmp_path / "runtime", ROOT, revision,
        environment="production",
    )
    text = result["runtime_env"].read_text()
    assert "ENVIRONMENT=production\n" in text
    assert "verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod" in text
    # Task 23: the six groups verify tokens and send one invite, so the
    # transaction projects that private set into the Rust file only.
    assert "JWT_SECRET=" + "k" * 40 + "\n" in text
    assert "JWT_ISSUER=verdaxis-prod-api\n" in text
    assert "JWT_AUDIENCE=verdaxis-prod-web\n" in text
    assert 'FRONTEND_URL="https://app.verdaxis.exchange"\n' in text
    assert 'EMAIL_FROM="Verdaxis <noreply@verdaxis.exchange>"\n' in text
    assert "RESEND_API_KEY=re_fixture_key\n" in text
    # An unrelated provider key never reaches the long-running process.
    assert "OPENAI_API_KEY" not in text
    assert "KYC_EXECUTION_GATE_ENABLED=false\n" in text
    env.write_text(env.read_text().replace("verdaxis_app:", "verdaxis_app_staging:"))
    with pytest.raises(preparer.RustRuntimeError, match="identity"):
        preparer.prepare(
            env, SHA, tmp_path / "runtime", ROOT, revision,
            environment="production",
        )
    # A partial Phase 2 projection is refused before the transaction commits.
    env.write_text(
        env.read_text()
        .replace("verdaxis_app_staging:", "verdaxis_app:")
        .replace("RESEND_API_KEY=re_fixture_key\n", "")
    )
    with pytest.raises(preparer.RustRuntimeError, match="RESEND_API_KEY"):
        preparer.prepare(
            env, SHA, tmp_path / "runtime", ROOT, revision,
            environment="production",
        )


@pytest.mark.parametrize("revision", ["kycg_20261014_staging", "prc_20261018_cache", "p0_20261016_union", "p3_20261020_auth_tokens", "p89_20261022_trade_freeze", "ob_20261031_offboarding", "*", "head", "unlisted_revision"])
def test_production_runtime_refuses_a_previous_or_unlisted_checkpoint(tmp_path, revision):
    preparer = _load_preparer()
    env = tmp_path / ".env"
    env.write_text(
        f"ENVIRONMENT=production\nRELEASE_SHA={SHA}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod\n"
    )
    with pytest.raises(preparer.RustRuntimeError):
        preparer.prepare(env, SHA, tmp_path / "runtime", ROOT, revision, environment="production")


class ProductionReleaseFixture(RustReleaseFixture):
    """Reuse the real transaction fixture, with separate production source."""

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
            "p89_20261022_trade_freeze\tp89_20261022_trade_freeze\n"
        )

    def push_rust_release(self, *, enabled):
        policy_path = self.source / "deploy/rust/production-policy.json"
        policy = json.loads(policy_path.read_text())
        policy["rust_enabled"] = enabled
        policy_path.write_text(json.dumps(policy))
        (self.source / "release.txt").write_text(str(enabled))
        _git(self.source, "add", "-A")
        _git(self.source, "commit", "-qm", "production release")
        _git(self.source, "push", "-q", "origin", "staging")
        return _git(self.source, "rev-parse", "HEAD").stdout.strip()

    def deploy_environment(self, release_sha, *, fail_phase=""):
        env = super().deploy_environment(release_sha, fail_phase=fail_phase)
        env.update(
            TARGET_BRANCH="staging", DEPLOY_ENVIRONMENT="production",
            SERVICE_NAME="verdaxis-backend.service",
            MIGRATION_EXPECTED_CURRENT_REVISION="p89_20261022_trade_freeze",
            MIGRATION_TARGET_REVISION="p89_20261022_trade_freeze",
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
        for tool in (systemctl, self.fake_bin / "sudo", self.fake_bin / "curl"):
            tool.write_text(tool.read_text().replace(
                "verdaxis-rust-staging.service", "verdaxis-rust.service"
            ).replace("8011", "8010"))

    def attest_from(self, ref, *, source_sha=None):
        """Rewrite and re-sign the fixture record as attested from `ref`."""
        record = self.artifact_root / "provenance-record.json"
        body = json.loads(record.read_text())
        body["workflow_ref"] = ref
        if source_sha is not None:
            body["source_sha"] = source_sha
        data = json.dumps(body).encode()
        record.write_bytes(data)
        (self.artifact_root / "provenance-record.sig").write_bytes(self.key.sign(data))

    def sign_for_prod(self):
        """Production accepts only the attestation made on the staging push."""
        self.attest_from("refs/heads/staging")


def refused_before_any_change(fixture, result):
    """The artifact refusal happens before source, state or service changes."""
    output = result.stdout + result.stderr
    assert result.returncode != 0, output
    assert "Rust artifact verification refused" in output
    assert _git(fixture.checkout, "rev-parse", "HEAD").stdout.strip() == fixture.old_sha
    assert not any(line.startswith("sudo:systemctl") for line in fixture.ledger)
    assert not (fixture.tmp_path / "runtime-deploy-state/production.state").exists()
    return output


def test_production_artifact_ref_is_the_staging_push_and_staging_is_unchanged():
    deploy = (ROOT / "scripts/deploy.sh").read_text()
    assert deploy.count('RUST_WORKFLOW_REF="refs/heads/staging"') == 1
    assert "refs/heads/prod" not in deploy


def test_production_sudoers_and_canonical_branch_are_exact():
    text = (ROOT / "deploy/sudoers/verdaxis-deploy-production").read_text()
    commands = {f"/usr/bin/systemctl {action} verdaxis-backend.service" for action in ("restart", "stop")}
    commands |= {f"/usr/bin/systemctl {action} verdaxis-rust.service" for action in ("restart", "stop", "disable", "enable")}
    actual = text.split("NOPASSWD: ", 1)[1].strip().split(", ")
    assert set(actual) == commands
    assert actual.count("/usr/bin/systemctl enable verdaxis-rust.service") == 1
    deploy = (ROOT / "scripts/deploy.sh").read_text()
    production = deploy.split("*/prod/be)", 1)[1].split(";;", 1)[0]
    assert 'DEFAULT_BRANCH="staging"' in production


def test_prod_publication_is_a_push_gate_not_a_pr_artifact():
    workflow = (ROOT / ".github/workflows/rust-read-ci.yml").read_text()
    assert "branches: [staging, prod]" in workflow
    assert "github.event_name == 'push'" in workflow
    assert "needs.offline.result == 'success'" in workflow
    assert "needs.contract.result == 'success'" in workflow
