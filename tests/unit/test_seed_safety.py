from pathlib import Path

import pytest

from app.seeds.safety import (
    SeedTargetError,
    attest_connected_seed_target,
    resolve_seed_target,
)


ROOT = Path(__file__).parents[2]
SEED_FILES = sorted((ROOT / "scripts").glob("*seed*.py"))


def _seed_env(**overrides: str) -> dict[str, str]:
    values = {
        "ALLOW_SEED_MUTATIONS": "I_UNDERSTAND_SEED_MUTATIONS",
        "SEED_RUNTIME_ENV": "disposable",
        "SEED_TARGET_DATABASE": "verdaxis_seed_test",
        "SEED_DATABASE_URL": (
            "postgresql://verdaxis_seed:runtime-only@127.0.0.1:55432/"
            "verdaxis_seed_test"
        ),
    }
    values.update(overrides)
    return values


def test_seed_target_requires_explicit_opt_in_and_attestation():
    with pytest.raises(SeedTargetError, match="SEED_DATABASE_URL"):
        resolve_seed_target({})
    with pytest.raises(SeedTargetError, match="ALLOW_SEED_MUTATIONS"):
        resolve_seed_target(_seed_env(ALLOW_SEED_MUTATIONS=""))
    with pytest.raises(SeedTargetError, match="SEED_TARGET_DATABASE"):
        resolve_seed_target(_seed_env(SEED_TARGET_DATABASE="wrong_test"))


@pytest.mark.parametrize(
    "overrides",
    [
        {"SEED_RUNTIME_ENV": "production"},
        {"SEED_TARGET_DATABASE": "verdaxis", "SEED_DATABASE_URL": "postgresql://verdaxis_seed:x@127.0.0.1/verdaxis"},
        {"SEED_RUNTIME_ENV": "disposable", "SEED_DATABASE_URL": "postgresql://verdaxis_seed:x@db.example/verdaxis_seed_test"},
        {"SEED_RUNTIME_ENV": "staging", "SEED_TARGET_DATABASE": "verdaxis_staging", "SEED_DATABASE_URL": "postgresql://verdaxis_seed:x@db.example/verdaxis_staging"},
        {"SEED_DATABASE_URL": "postgresql://postgres:x@127.0.0.1/verdaxis_seed_test"},
    ],
)
def test_seed_target_denies_production_unattested_or_privileged_targets(overrides):
    with pytest.raises(SeedTargetError):
        resolve_seed_target(_seed_env(**overrides))


def test_seed_target_accepts_exact_disposable_and_staging_targets():
    disposable = resolve_seed_target(_seed_env())
    assert disposable.database_name == "verdaxis_seed_test"
    assert disposable.runtime_env == "disposable"

    staging = resolve_seed_target(
        _seed_env(
            SEED_RUNTIME_ENV="staging",
            SEED_TARGET_DATABASE="verdaxis_staging",
            SEED_DATABASE_URL="postgresql://verdaxis_seed:x@127.0.0.1/verdaxis_staging",
        )
    )
    assert staging.database_name == "verdaxis_staging"


def test_connected_seed_identity_must_match_and_must_not_be_superuser():
    target = resolve_seed_target(_seed_env())
    attest_connected_seed_target(
        target,
        connected_database="verdaxis_seed_test",
        connected_user="verdaxis_seed",
        connected_role_is_superuser=False,
    )
    with pytest.raises(SeedTargetError, match="database does not match"):
        attest_connected_seed_target(
            target,
            connected_database="verdaxis",
            connected_user="verdaxis_seed",
            connected_role_is_superuser=False,
        )
    with pytest.raises(SeedTargetError, match="current_user does not match"):
        attest_connected_seed_target(
            target,
            connected_database="verdaxis_seed_test",
            connected_user="unexpected_role",
            connected_role_is_superuser=False,
        )
    with pytest.raises(SeedTargetError, match="superuser"):
        attest_connected_seed_target(
            target,
            connected_database="verdaxis_seed_test",
            connected_user="verdaxis_seed",
            connected_role_is_superuser=True,
        )


def test_all_executable_seeders_use_the_shared_fail_closed_target_gate():
    assert SEED_FILES
    for path in SEED_FILES:
        source = path.read_text()
        assert "app.seeds.safety" in source, path
        assert "SEED_DATABASE_URL" not in source, path


def test_seeders_contain_no_static_database_credentials_or_legacy_windows():
    forbidden = (
        "password=\"",
        "password='",
        '"password":',
        "'password':",
        "user=\"postgres\"",
        "user='postgres'",
        '"Spot"',
        '"Q2 2026"',
        '"Q3 2026"',
        '"Q4 2026"',
        '"Forward 2027"',
    )
    for path in [*SEED_FILES, *(ROOT / "app/seeds").glob("*.py")]:
        source = path.read_text()
        assert not any(value in source for value in forbidden), path
