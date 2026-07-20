"""Attested deployment migration checkpoint contract."""

from __future__ import annotations

import asyncio
import importlib.util
from pathlib import Path
from types import SimpleNamespace

import pytest
from alembic.config import Config
from alembic.script import ScriptDirectory


ROOT = Path(__file__).parents[2]
SOURCE_SHA = "a" * 40
EXPECTED = "pa_20260715_analytics_facts"
TARGET = "rh_20260720_runtime_metadata"


def _load_checkpoint_module():
    path = ROOT / "scripts/apply_migration_checkpoint.py"
    assert path.exists(), "attested migration checkpoint helper is required"
    spec = importlib.util.spec_from_file_location("apply_migration_checkpoint", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_checkpoint_policy_is_explicit_and_rejects_symbolic_targets():
    module = _load_checkpoint_module()

    policy = module.parse_checkpoint_policy(
        f"{EXPECTED}\t{TARGET}\n{TARGET}\t{TARGET}\n"
    )

    assert policy == {(EXPECTED, TARGET), (TARGET, TARGET)}
    for alias in ("head", "heads", "base", "+1", "-1"):
        with pytest.raises(module.MigrationCheckpointError):
            module.parse_checkpoint_policy(f"{EXPECTED}\t{alias}\n")


def test_checkpoint_request_binds_sha_current_target_and_real_graph():
    module = _load_checkpoint_module()
    script = ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))
    policy = {(EXPECTED, TARGET)}

    module.validate_checkpoint_request(
        policy=policy,
        source_sha=SOURCE_SHA,
        approved_source_sha=SOURCE_SHA,
        expected_current=EXPECTED,
        target=TARGET,
        current_heads=(EXPECTED,),
        script_directory=script,
    )

    with pytest.raises(module.MigrationCheckpointError, match="source SHA"):
        module.validate_checkpoint_request(
            policy=policy,
            source_sha=SOURCE_SHA,
            approved_source_sha="b" * 40,
            expected_current=EXPECTED,
            target=TARGET,
            current_heads=(EXPECTED,),
            script_directory=script,
        )
    with pytest.raises(module.MigrationCheckpointError, match="current revision"):
        module.validate_checkpoint_request(
            policy=policy,
            source_sha=SOURCE_SHA,
            approved_source_sha=SOURCE_SHA,
            expected_current=EXPECTED,
            target=TARGET,
            current_heads=(TARGET,),
            script_directory=script,
        )
    with pytest.raises(module.MigrationCheckpointError, match="allowlisted"):
        module.validate_checkpoint_request(
            policy=policy,
            source_sha=SOURCE_SHA,
            approved_source_sha=SOURCE_SHA,
            expected_current=EXPECTED,
            target="pref_20260709_user_preferences",
            current_heads=(EXPECTED,),
            script_directory=script,
        )
    with pytest.raises(module.MigrationCheckpointError, match="descendant"):
        module.validate_checkpoint_request(
            policy={(EXPECTED, "pref_20260709_user_preferences")},
            source_sha=SOURCE_SHA,
            approved_source_sha=SOURCE_SHA,
            expected_current=EXPECTED,
            target="pref_20260709_user_preferences",
            current_heads=(EXPECTED,),
            script_directory=script,
        )


def test_checkpoint_executor_passes_only_literal_target_to_alembic(monkeypatch):
    module = _load_checkpoint_module()
    observed_heads = iter(((EXPECTED,), (TARGET,)))
    upgrade_targets: list[str] = []

    async def fake_current_heads(_settings):
        return next(observed_heads)

    monkeypatch.setattr(module, "_read_current_heads", fake_current_heads)
    monkeypatch.setattr(
        module.command,
        "upgrade",
        lambda _config, revision: upgrade_targets.append(revision),
    )

    asyncio.run(
        module.execute_checkpoint(
            config=Config(str(ROOT / "alembic.ini")),
            settings=SimpleNamespace(
                RELEASE_SHA=SOURCE_SHA,
                DATABASE_URL=(
                    "postgresql+asyncpg://verdaxis_app:app-secret@db/verdaxis"
                ),
                MIGRATOR_DATABASE_URL=(
                    "postgresql+asyncpg://verdaxis_migrator:"
                    "migration-secret@db/verdaxis"
                ),
            ),
            policy={(EXPECTED, TARGET)},
            source_sha=SOURCE_SHA,
            approved_source_sha=SOURCE_SHA,
            expected_current=EXPECTED,
            target=TARGET,
        )
    )

    assert upgrade_targets == [TARGET]
    assert "head" not in upgrade_targets


def test_checkpoint_git_identity_uses_absolute_trusted_binary():
    source = (ROOT / "scripts/apply_migration_checkpoint.py").read_text()

    assert '"/usr/bin/git"' in source
    assert '\n            "git",' not in source
    assert 'environment["GIT_CONFIG_GLOBAL"] = "/dev/null"' in source
    assert 'environment["GIT_CONFIG_NOSYSTEM"] = "1"' in source


def test_checkpoint_executor_pins_alembic_to_explicit_migrator_url(monkeypatch):
    module = _load_checkpoint_module()
    migration_url = (
        "postgresql+asyncpg://verdaxis_migrator:migration-secret@db/verdaxis"
    )
    app_url = "postgresql+asyncpg://verdaxis_app:app-secret@db/verdaxis"
    settings = SimpleNamespace(
        RELEASE_SHA=SOURCE_SHA,
        DATABASE_URL=app_url,
        MIGRATOR_DATABASE_URL=migration_url,
    )
    observed_heads = iter(((EXPECTED,), (TARGET,)))
    observed_urls: list[str] = []

    async def fake_current_heads(_settings):
        return next(observed_heads)

    def capture_upgrade(config, revision):
        assert revision == TARGET
        observed_urls.append(config.get_main_option("sqlalchemy.url"))

    monkeypatch.setattr(module, "_read_current_heads", fake_current_heads)
    monkeypatch.setattr(module.command, "upgrade", capture_upgrade)
    config = Config(str(ROOT / "alembic.ini"))

    asyncio.run(
        module.execute_checkpoint(
            config=config,
            settings=settings,
            policy={(EXPECTED, TARGET)},
            source_sha=SOURCE_SHA,
            approved_source_sha=SOURCE_SHA,
            expected_current=EXPECTED,
            target=TARGET,
        )
    )

    assert observed_urls == [migration_url]
    assert app_url not in observed_urls


def test_checkpoint_executor_refuses_app_credential_before_alembic(monkeypatch):
    module = _load_checkpoint_module()
    app_url = "postgresql+asyncpg://verdaxis_app:app-secret@db/verdaxis"
    settings = SimpleNamespace(
        RELEASE_SHA=SOURCE_SHA,
        DATABASE_URL=app_url,
        MIGRATOR_DATABASE_URL=app_url,
    )

    async def forbidden_current_heads(_settings):
        pytest.fail("checkpoint read started before credential separation")

    def forbidden_upgrade(_config, _revision):
        pytest.fail("Alembic ran with the application credential")

    monkeypatch.setattr(module, "_read_current_heads", forbidden_current_heads)
    monkeypatch.setattr(module.command, "upgrade", forbidden_upgrade)

    with pytest.raises(module.MigrationCheckpointError, match="distinct .* role"):
        asyncio.run(
            module.execute_checkpoint(
                config=Config(str(ROOT / "alembic.ini")),
                settings=settings,
                policy={(EXPECTED, TARGET)},
                source_sha=SOURCE_SHA,
                approved_source_sha=SOURCE_SHA,
                expected_current=EXPECTED,
                target=TARGET,
            )
        )


def test_runtime_revision_verifier_requires_exact_deployed_checkpoint(monkeypatch):
    module = _load_checkpoint_module()

    async def exact_heads(_settings):
        return (TARGET,)

    monkeypatch.setattr(module, "_read_current_heads", exact_heads)
    asyncio.run(module.verify_current_revision(SimpleNamespace(), TARGET))

    async def later_heads(_settings):
        return ("future_security_head",)

    monkeypatch.setattr(module, "_read_current_heads", later_heads)
    with pytest.raises(module.MigrationCheckpointError, match="runtime migration"):
        asyncio.run(module.verify_current_revision(SimpleNamespace(), TARGET))


@pytest.mark.parametrize(
    "environment", ["development", "test", "staging", "production"]
)
@pytest.mark.parametrize("migrator_url", [None, ""])
def test_checkpoint_reader_never_falls_back_to_app_url(
    monkeypatch, environment, migrator_url
):
    module = _load_checkpoint_module()

    def forbidden_engine(*_args, **_kwargs):
        pytest.fail("checkpoint reader opened an engine before URL attestation")

    monkeypatch.setattr(module, "create_async_engine", forbidden_engine)
    settings = SimpleNamespace(
        ENVIRONMENT=environment,
        DATABASE_URL="postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        MIGRATOR_DATABASE_URL=migrator_url,
    )

    with pytest.raises(
        module.MigrationCheckpointError,
        match="explicit MIGRATOR_DATABASE_URL",
    ):
        asyncio.run(module._read_current_heads(settings))


@pytest.mark.parametrize(
    "migrator_url",
    [
        "postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        "postgresql+asyncpg://verdaxis_app:different@other-db/verdaxis",
    ],
)
def test_checkpoint_reader_requires_distinct_app_and_migrator_authority(
    monkeypatch, migrator_url
):
    module = _load_checkpoint_module()

    def forbidden_engine(*_args, **_kwargs):
        pytest.fail("checkpoint reader opened an engine before URL attestation")

    monkeypatch.setattr(module, "create_async_engine", forbidden_engine)
    settings = SimpleNamespace(
        ENVIRONMENT="production",
        DATABASE_URL="postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        MIGRATOR_DATABASE_URL=migrator_url,
    )

    with pytest.raises(
        module.MigrationCheckpointError,
        match="distinct .* role",
    ):
        asyncio.run(module._read_current_heads(settings))


def test_checkpoint_reader_requires_same_app_and_migrator_endpoint(monkeypatch):
    module = _load_checkpoint_module()

    def forbidden_engine(*_args, **_kwargs):
        pytest.fail("checkpoint reader opened an engine before endpoint attestation")

    monkeypatch.setattr(module, "create_async_engine", forbidden_engine)
    settings = SimpleNamespace(
        ENVIRONMENT="production",
        DATABASE_URL="postgresql+asyncpg://verdaxis_app:app@db-a/verdaxis",
        MIGRATOR_DATABASE_URL=(
            "postgresql+asyncpg://verdaxis_migrator:migrator@db-b/verdaxis"
        ),
    )

    with pytest.raises(module.MigrationCheckpointError, match="same database endpoint"):
        asyncio.run(module._read_current_heads(settings))


def test_deploy_contains_no_unconditional_head_traversal():
    source = (ROOT / "scripts/deploy.sh").read_text()

    assert "alembic upgrade head" not in source
    assert "MIGRATION_APPROVED_SOURCE_SHA" in source
    assert "MIGRATION_EXPECTED_CURRENT_REVISION" in source
    assert "MIGRATION_TARGET_REVISION" in source
    assert "MIGRATION_REVISION=" in source
    assert "scripts/apply_migration_checkpoint.py" in source
    verifier = ROOT / "scripts/verify_migration_revision.py"
    assert verifier.exists()
    assert "verify_current_revision" in verifier.read_text()


def test_deploy_converges_acl_policy_after_migration_before_restart():
    source = (ROOT / "scripts/deploy.sh").read_text()

    migration = source.index("scripts/apply_migration_checkpoint.py")
    convergence = source.index(
        '"$ACL_CONVERGENCE_TEMP/$ACL_CONVERGENCE_HELPER_PATH"'
    )
    restart = source.index('sudo systemctl restart "$SERVICE_NAME"')

    assert migration < convergence < restart
    assert "deploy/postgres/converge_runtime_object_acls.sql" in source
    assert "deploy/postgres/app_acl_policy.sql" in source
    assert "bootstrap_roles.sql" not in source
