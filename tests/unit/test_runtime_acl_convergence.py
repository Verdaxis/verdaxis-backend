"""Steady-state runtime ACL convergence boundary."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest


ROOT = Path(__file__).parents[2]


def _load_convergence_module():
    path = ROOT / "scripts/converge_runtime_acls.py"
    assert path.exists(), "owner-executable runtime ACL convergence is required"
    spec = importlib.util.spec_from_file_location("converge_runtime_acls", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize(
    ("environment", "database", "app_role", "migrator_role", "backup_role"),
    [
        (
            "production",
            "verdaxis",
            "verdaxis_app",
            "verdaxis_migrator",
            "verdaxis_backup",
        ),
        (
            "staging",
            "verdaxis_staging",
            "verdaxis_app_staging",
            "verdaxis_migrator_staging",
            "verdaxis_backup_staging",
        ),
    ],
)
def test_acl_target_is_bound_to_exact_deployed_identity(
    environment, database, app_role, migrator_role, backup_role
):
    module = _load_convergence_module()
    values = {
        "DATABASE_URL": (
            f"postgresql+asyncpg://{app_role}:app-secret@db.internal/{database}"
        ),
        "MIGRATOR_DATABASE_URL": (
            f"postgresql+asyncpg://{migrator_role}:migration-secret@db.internal/"
            f"{database}"
        ),
    }

    target = module.resolve_acl_target(environment, values)

    assert target.database_name == database
    assert target.app_role == app_role
    assert target.migrator_role == migrator_role
    assert target.backup_role == backup_role


@pytest.mark.parametrize("migrator_url", [None, ""])
def test_acl_target_never_falls_back_to_app_url(migrator_url):
    module = _load_convergence_module()
    values = {
        "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        "MIGRATOR_DATABASE_URL": migrator_url,
    }

    with pytest.raises(module.RuntimeAclConvergenceError, match="explicit"):
        module.resolve_acl_target("production", values)


def test_acl_target_rejects_shared_app_and_migrator_role():
    module = _load_convergence_module()
    values = {
        "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        "MIGRATOR_DATABASE_URL": (
            "postgresql+asyncpg://verdaxis_app:different@other-db/verdaxis"
        ),
    }

    with pytest.raises(module.RuntimeAclConvergenceError, match="migrator"):
        module.resolve_acl_target("production", values)


def test_acl_target_rejects_different_app_and_migrator_endpoint():
    module = _load_convergence_module()
    values = {
        "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:app@db-a/verdaxis",
        "MIGRATOR_DATABASE_URL": (
            "postgresql+asyncpg://verdaxis_migrator:migrator@db-b/verdaxis"
        ),
    }

    with pytest.raises(
        module.RuntimeAclConvergenceError,
        match="same database endpoint",
    ):
        module.resolve_acl_target("production", values)


def test_psql_invocation_uses_single_transaction_without_password_in_argv(tmp_path):
    module = _load_convergence_module()
    values = {
        "DATABASE_URL": "postgresql+asyncpg://verdaxis_app:app@db/verdaxis",
        "MIGRATOR_DATABASE_URL": (
            "postgresql+asyncpg://verdaxis_migrator:migration-secret@db/verdaxis"
        ),
    }
    target = module.resolve_acl_target("production", values)

    command, process_environment = module.build_psql_invocation(tmp_path, target)

    assert command[0] == "/usr/bin/psql"
    assert "--single-transaction" in command
    assert "migration-secret" not in " ".join(command)
    assert process_environment["PGPASSWORD"] == "migration-secret"
    assert process_environment["PATH"] == "/usr/bin:/bin"
    assert f"database_name={target.database_name}" in command
    assert f"app_role={target.app_role}" in command
    assert f"migrator_role={target.migrator_role}" in command
    assert f"backup_role={target.backup_role}" in command


def test_environment_file_loader_refuses_symlink_without_reading_target(tmp_path):
    module = _load_convergence_module()
    target = tmp_path / "operator-secrets"
    target.write_text(
        "DATABASE_URL=postgresql://verdaxis_app:secret@db/verdaxis\n"
        "MIGRATOR_DATABASE_URL="
        "postgresql://verdaxis_migrator:secret@db/verdaxis\n"
    )
    environment_file = tmp_path / ".env"
    environment_file.symlink_to(target)

    with pytest.raises(
        module.RuntimeAclConvergenceError,
        match="unable to read the deployed environment file",
    ):
        module._load_database_values(environment_file)


def test_environment_file_loader_reads_regular_file(tmp_path, monkeypatch):
    module = _load_convergence_module()
    monkeypatch.delenv("DATABASE_URL", raising=False)
    monkeypatch.delenv("MIGRATOR_DATABASE_URL", raising=False)
    environment_file = tmp_path / ".env"
    environment_file.write_text(
        "DATABASE_URL=postgresql://verdaxis_app:app@db/verdaxis\n"
        "MIGRATOR_DATABASE_URL="
        "postgresql://verdaxis_migrator:migrator@db/verdaxis\n"
    )

    values = module._load_database_values(environment_file)

    assert values == {
        "DATABASE_URL": "postgresql://verdaxis_app:app@db/verdaxis",
        "MIGRATOR_DATABASE_URL": (
            "postgresql://verdaxis_migrator:migrator@db/verdaxis"
        ),
    }


def test_environment_file_loader_ignores_ambient_database_url_overrides(
    tmp_path, monkeypatch
):
    module = _load_convergence_module()
    environment_file = tmp_path / ".env"
    environment_file.write_text(
        "DATABASE_URL=postgresql://verdaxis_app:file-app@db/verdaxis\n"
        "MIGRATOR_DATABASE_URL="
        "postgresql://verdaxis_migrator:file-migrator@db/verdaxis\n"
    )
    monkeypatch.setenv(
        "DATABASE_URL",
        "postgresql://verdaxis_app:ambient-app@decoy/verdaxis",
    )
    monkeypatch.setenv(
        "MIGRATOR_DATABASE_URL",
        "postgresql://verdaxis_migrator:ambient-migrator@decoy/verdaxis",
    )

    values = module._load_database_values(environment_file)

    assert values["DATABASE_URL"] == (
        "postgresql://verdaxis_app:file-app@db/verdaxis"
    )
    assert values["MIGRATOR_DATABASE_URL"] == (
        "postgresql://verdaxis_migrator:file-migrator@db/verdaxis"
    )
