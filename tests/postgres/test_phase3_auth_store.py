"""Phase 3 auth storage on PostgreSQL 17.

Covers the additive reset-token migration and its single Alembic head, the
runtime ACL of the new table, the device advisory-lock key Rust and Python
share, and the Rust store and lock primitives on a migrated database
(`rs/read/tests/auth_store_postgres.rs`).
"""
from __future__ import annotations

import asyncio
import importlib.util
import json
import os
import re
import subprocess
from pathlib import Path
from uuid import uuid4

import pytest
from alembic.config import Config
from alembic.script import ScriptDirectory
from sqlalchemy import text
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.test_market_migrations import (  # noqa: F401
    _alembic,
    _database_execute,
    migration_database,
)

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase3"
RECORD = json.loads((FIXTURES / "migration.json").read_text())
REVISION = RECORD["revision"]
PARENT = RECORD["parent"]
TABLE = RECORD["table"]
STORE_SOURCE = ROOT / "rs/read/src/auth/store.rs"

# Every PostgreSQL test in rs/read/tests/auth_store_postgres.rs. Each must run
# and pass; a skipped or missing test fails here.
RUST_POSTGRES_TESTS = (
    "auth_lock_order_serializes_device_user_refresh",
    "user_rows_lock_by_id_and_by_normalized_email",
    "refresh_rows_insert_lock_and_revoke",
    "reset_tokens_replace_and_consume_once",
    "reset_token_issuance_keeps_other_users_independent",
    "expired_reset_cleanup_skips_locked_rows",
    "organization_write_locks_follow_one_order",
    "org_write_retry_classifies_postgres_conflicts",
)

# Session lock order: device lock, then the user row, then that user's
# refresh-session or reset-token rows. A function may skip a step but never
# go back to an earlier one.
SESSION_LOCK_RANK = {
    "lock_device": 0,
    "lock_user": 1,
    "lock_user_by_email": 1,
    "lock_refresh_by_jti": 2,
    "revoke_device_families": 2,
    "revoke_user_families": 2,
    "insert_refresh": 2,
    "upsert_reset_token": 2,
    "consume_reset_token": 2,
}
SESSION_LOCK_CALL = re.compile(
    r"(?<![\w.])(?<!fn )(" + "|".join(SESSION_LOCK_RANK) + r")\s*\("
)

# One row per table, schema object or ACL entry; every value is text.
SCHEMA_SNAPSHOT = """
SELECT 'table', tablename::text, tablename::text, ''
FROM pg_catalog.pg_tables WHERE schemaname = 'public'
UNION ALL
SELECT 'column', table_name::text, column_name::text,
       data_type::text || ' ' || is_nullable::text || ' ' || coalesce(column_default::text, '')
FROM information_schema.columns WHERE table_schema = 'public'
UNION ALL
SELECT 'index', tablename::text, indexname::text, indexdef
FROM pg_catalog.pg_indexes WHERE schemaname = 'public'
UNION ALL
SELECT 'constraint', conrelid::regclass::text, conname::text, pg_get_constraintdef(oid)
FROM pg_catalog.pg_constraint WHERE connamespace = 'public'::regnamespace
UNION ALL
SELECT 'trigger', tgrelid::regclass::text, tgname::text, pg_get_triggerdef(oid)
FROM pg_catalog.pg_trigger WHERE NOT tgisinternal
"""

TABLE_ACL = """
SELECT coalesce(grantee.rolname, 'PUBLIC'), acl.privilege_type
FROM pg_catalog.pg_class AS object
CROSS JOIN LATERAL aclexplode(object.relacl) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE object.oid = 'public.password_reset_tokens'::regclass
  AND acl.grantee <> object.relowner
"""

COLUMN_ACL = """
SELECT coalesce(grantee.rolname, 'PUBLIC'), attribute.attname::text, acl.privilege_type
FROM pg_catalog.pg_attribute AS attribute
CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE attribute.attrelid = 'public.password_reset_tokens'::regclass
  AND attribute.attnum > 0 AND NOT attribute.attisdropped
"""


def _script() -> ScriptDirectory:
    return ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))


def _parents(revision) -> tuple[str, ...]:
    down = revision.down_revision
    if down is None:
        return ()
    return (down,) if isinstance(down, str) else tuple(down)


def _checkpoint_module():
    path = ROOT / "scripts/apply_migration_checkpoint.py"
    spec = importlib.util.spec_from_file_location("apply_migration_checkpoint", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


async def _schema(database_url: str) -> set[tuple[str, str, str, str]]:
    result = await _database_execute(database_url, SCHEMA_SNAPSHOT)
    return {tuple(row) for row in result.all()}


def _libpq(url: str) -> str:
    return url.replace("postgresql+asyncpg://", "postgres://", 1)


def test_device_lock_key_matches_python():
    from app.core.security import hash_token_identifier
    from app.routers.auth_simple import (
        DEVICE_SESSION_ID_PATTERN,
        _device_advisory_lock_key,
    )

    vectors = json.loads((FIXTURES / "device-lock-keys.json").read_text())
    cookies = vectors["cookies"]
    assert len(cookies) == 20
    assert any(vector["key"] < 0 for vector in cookies)
    assert any(vector["key"] >= 0 for vector in cookies)
    for vector in cookies:
        assert DEVICE_SESSION_ID_PATTERN.fullmatch(vector["cookie"])
        assert hash_token_identifier(vector["cookie"]) == vector["digest"]
        assert _device_advisory_lock_key(vector["digest"]) == vector["key"]
    for vector in vectors["digests"]:
        assert _device_advisory_lock_key(vector["digest"]) == vector["key"], vector


def test_phase3_migration_has_one_head_and_direct_parent():
    script = _script()
    assert PARENT == RECORD["observed_single_head_at_task_start"]
    assert script.get_heads() == ["p5_20261020_read_indexes"]
    assert script.get_revision("p5_20261020_read_indexes").down_revision == REVISION
    revision = script.get_revision(REVISION)
    assert revision.down_revision == PARENT
    assert revision.module.branch_labels is None
    assert revision.module.depends_on is None
    children = [
        candidate.revision
        for candidate in script.walk_revisions()
        if PARENT in _parents(candidate)
    ]
    assert children == [REVISION]


@pytest.mark.asyncio
async def test_phase3_migration_is_additive_and_reversible(migration_database):
    database_url, _database_name = migration_database
    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", PARENT)
    assert upgraded.returncode == 0, upgraded.stderr
    before = await _schema(database_url)

    upgraded = await asyncio.to_thread(_alembic, database_url, "upgrade", REVISION)
    assert upgraded.returncode == 0, upgraded.stderr
    after = await _schema(database_url)
    assert before <= after, "the migration changed an existing object"
    added = after - before
    assert {table for _kind, table, _name, _definition in added} == {TABLE}
    columns = {(name, definition) for kind, _t, name, definition in added if kind == "column"}
    assert columns == {
        (
            column["name"],
            f"{column['type']} {'YES' if column['nullable'] else 'NO'} {column['default'] or ''}",
        )
        for column in RECORD["columns"]
    }
    constraints = {
        (name, definition) for kind, _t, name, definition in added if kind == "constraint"
    }
    assert constraints == set(RECORD["constraints"].items())
    indexes = {name for kind, _t, name, _d in added if kind == "index"}
    assert indexes == {*RECORD["indexes"], *RECORD["constraints"]} - {
        name for name, definition in RECORD["constraints"].items()
        if definition.startswith("FOREIGN KEY")
    }
    assert not {row for row in added if row[0] == "trigger"}

    downgraded = await asyncio.to_thread(_alembic, database_url, "downgrade", PARENT)
    assert downgraded.returncode == 0, downgraded.stderr
    assert await _schema(database_url) == before

    module = _checkpoint_module()
    policy = module.parse_checkpoint_policy(
        (ROOT / "deploy/migration-checkpoints.tsv").read_text()
    )
    rows = {tuple(row) for row in RECORD["checkpoint_rows"]}
    assert {row for row in policy if REVISION in row} == rows | {
        (REVISION, "p5_20261020_read_indexes")
    }
    script = _script()
    path = [revision.revision for revision in script.iterate_revisions(REVISION, PARENT)]
    assert path == [REVISION]
    for revision in path:
        assert (revision, revision) in policy
    for current, target in rows:
        module.validate_checkpoint_request(
            policy=policy,
            source_sha="a" * 40,
            approved_source_sha="a" * 40,
            expected_current=current,
            target=target,
            current_heads=(current,),
            script_directory=script,
        )


@pytest.mark.asyncio
async def test_password_reset_tokens_are_app_writable_and_other_roles_denied(analytics_pg_url):
    app_role = os.environ["RUNTIME_TEST_APP_ROLE"]
    backup_role = os.environ["RUNTIME_TEST_BACKUP_ROLE"]
    expected = RECORD["runtime_privileges"]
    admin = create_async_engine(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    app = create_async_engine(os.environ["DATABASE_URL"])
    try:
        async with admin.connect() as connection:
            table_acl = set((await connection.execute(text(TABLE_ACL))).all())
            column_acl = set((await connection.execute(text(COLUMN_ACL))).all())
        assert table_acl == {
            *((app_role, privilege) for privilege in expected["table"]),
            (backup_role, "SELECT"),
        }
        assert column_acl == {
            *((app_role, column, "INSERT") for column in expected["insert_columns"]),
            *((app_role, column, "UPDATE") for column in expected["update_columns"]),
        }

        # The runtime role reads, writes and deletes its rows.
        user_id, token_hash = uuid4(), uuid4().hex * 2
        async with app.connect() as connection:
            transaction = await connection.begin()
            await connection.execute(
                text(
                    "INSERT INTO users (id, email, password_hash, role, status, "
                    "email_verified, must_change_password) VALUES "
                    "(:id, :email, 'not-a-login', 'BUYER', 'APPROVED', true, false)"
                ),
                {"id": user_id, "email": f"reset-acl-{user_id}@phase3.invalid"},
            )
            await connection.execute(
                text(
                    "INSERT INTO password_reset_tokens (user_id, token_hash, expires_at) "
                    "VALUES (:id, :hash, now() + interval '1 hour')"
                ),
                {"id": user_id, "hash": token_hash},
            )
            stored = await connection.scalar(
                text("SELECT token_hash FROM password_reset_tokens WHERE user_id = :id"),
                {"id": user_id},
            )
            assert stored == token_hash
            updated = await connection.execute(
                text("UPDATE password_reset_tokens SET used_at = now() WHERE user_id = :id"),
                {"id": user_id},
            )
            assert updated.rowcount == 1
            deleted = await connection.execute(
                text("DELETE FROM password_reset_tokens WHERE user_id = :id"),
                {"id": user_id},
            )
            assert deleted.rowcount == 1
            await transaction.rollback()

        # A role without a policy row sees nothing, and a column added later
        # gets no write authority from the explicit column policy.
        async with admin.connect() as connection:
            transaction = await connection.begin()
            await connection.execute(text("CREATE ROLE p3_reset_token_probe NOLOGIN"))
            probe = (
                await connection.execute(
                    text(
                        "SELECT has_table_privilege('p3_reset_token_probe', "
                        "'public.password_reset_tokens', 'SELECT'), "
                        "has_any_column_privilege('p3_reset_token_probe', "
                        "'public.password_reset_tokens', 'SELECT')"
                    )
                )
            ).one()
            assert tuple(probe) == (False, False)
            await connection.execute(
                text("ALTER TABLE public.password_reset_tokens ADD COLUMN later_column text")
            )
            later = (
                await connection.execute(
                    text(
                        "SELECT has_column_privilege(:role, 'public.password_reset_tokens', "
                        "'later_column', 'INSERT'), has_column_privilege(:role, "
                        "'public.password_reset_tokens', 'later_column', 'UPDATE')"
                    ),
                    {"role": app_role},
                )
            ).one()
            assert tuple(later) == (False, False)
            await connection.execute(text(f'SET LOCAL ROLE "{app_role}"'))
            with pytest.raises(DBAPIError) as refused:
                await connection.execute(
                    text("UPDATE public.password_reset_tokens SET later_column = 'x' WHERE false")
                )
            assert getattr(refused.value.orig, "sqlstate", None) == "42501"
            await transaction.rollback()
    finally:
        await app.dispose()
        await admin.dispose()


@pytest.fixture(scope="module")
def rust_store_run(analytics_pg_url):
    """Run the Rust store tests once against the migrated, ACL-converged database."""
    environment = {
        **os.environ,
        "AUTH_STORE_TEST_DATABASE_URL": _libpq(os.environ["DATABASE_URL"]),
        "AUTH_STORE_TEST_OWNER_URL": _libpq(analytics_pg_url),
        "RELEASE_SHA": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "SQLX_OFFLINE": "true",
    }
    return subprocess.run(
        [
            "cargo", "test", "--manifest-path", "rs/Cargo.toml", "--locked",
            "-p", "verdaxis-read", "--test", "auth_store_postgres", "--", "--include-ignored",
        ],
        cwd=ROOT,
        env=environment,
        capture_output=True,
        text=True,
        timeout=1800,
    )


def _assert_rust_passed(run: subprocess.CompletedProcess[str], name: str) -> None:
    assert run.returncode == 0, run.stdout[-6000:] + run.stderr[-6000:]
    assert f"test {name} ... ok" in run.stdout, name


def test_rust_auth_store_on_migrated_database(rust_store_run):
    for name in RUST_POSTGRES_TESTS:
        _assert_rust_passed(rust_store_run, name)


def test_auth_lock_order_serializes_device_user_refresh(rust_store_run):
    _assert_rust_passed(rust_store_run, "auth_lock_order_serializes_device_user_refresh")
    sources = sorted((ROOT / "rs/read/src").rglob("*.rs"))
    assert STORE_SOURCE in sources
    for path in sources:
        source = re.sub(r"//[^\n]*", "", path.read_text())
        if path != STORE_SOURCE:
            # Session and reset-token rows are touched only through the store.
            assert "refresh_sessions" not in source, path
            assert "password_reset_tokens" not in source, path
        for function in re.split(r"\bfn\s+\w+", source):
            ranks = [SESSION_LOCK_RANK[name] for name in SESSION_LOCK_CALL.findall(function)]
            assert ranks == sorted(ranks), f"{path}: session locks out of order"
