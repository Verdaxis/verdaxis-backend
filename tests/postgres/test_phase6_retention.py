"""Disposable PostgreSQL proofs for the durable replay floor schema.

Neutral scenario IDs E007-E031 of
``tests/rust_contract/phase6/required-scenarios.json``. The proofs use the
deployed app/migrator/backup role split on a disposable PostgreSQL 17/PostGIS
cluster; nothing here targets a live database.

The read store itself is proved by ``rs/read/tests/streams_store.rs``. This
module seeds its fixtures and invokes the named store tests, so the page SQL and
the schema contract are exercised together.
"""

from __future__ import annotations

import asyncio
import json
import os
import subprocess
import sys
import time
from pathlib import Path
from uuid import UUID, uuid4

import pytest
from alembic.config import Config
from alembic.script import ScriptDirectory
from sqlalchemy import text
from sqlalchemy.engine import make_url
from sqlalchemy.exc import DBAPIError
from sqlalchemy.ext.asyncio import create_async_engine

from tests.postgres.retention_test_support import (
    PARENT_REVISION,
    REVISION,
    clear_retention,
    record_ownership,
)

ROOT = Path(__file__).resolve().parents[2]
ORG = UUID("00000000-0000-4000-8000-0000000000a1")
FOREIGN_ORG = UUID("00000000-0000-4000-8000-0000000000b2")
TABLES = ("market_event_replay_state", "market_event_retention_control")
FUNCTIONS = (
    "market_event_outbox_advance_replay_floor",
    "market_event_outbox_refuse_truncate",
)


def _require(name: str) -> str:
    value = os.environ.get(name, "").strip()
    if not value:
        pytest.skip(f"{name} is not configured")
    return value


def _sqlstate(error: DBAPIError) -> str | None:
    original = error.orig
    return getattr(original, "sqlstate", None) or getattr(original, "pgcode", None)


async def _expect_error(engine, statement: str, *, code: str | None = None, params=None) -> DBAPIError:
    with pytest.raises(DBAPIError) as caught:
        async with engine.begin() as connection:
            await connection.execute(text(statement), params or {})
    if code is not None:
        assert _sqlstate(caught.value) == code, caught.value
    return caught.value


async def _insert_event(
    engine,
    *,
    seq: int | None,
    participants,
    event_type: str = "trade_created",
    payload: dict | None = None,
) -> UUID:
    identifier = uuid4()
    async with engine.begin() as connection:
        await connection.execute(
            text(
                """
                INSERT INTO public.market_event_outbox
                    (id, event_type, aggregate_type, aggregate_id,
                     participant_org_ids, payload, created_at, stream_seq)
                VALUES
                    (:id, :event_type, 'trade', :aggregate,
                     CAST(:participants AS json), CAST(:payload AS json), now(), :seq)
                """
            ),
            {
                "id": identifier,
                "event_type": event_type,
                "aggregate": str(identifier),
                "participants": json.dumps([str(value) for value in participants]),
                "payload": json.dumps(payload if payload is not None else {"schema_version": 1}),
                "seq": seq,
            },
        )
    return identifier


async def _floor(engine) -> int:
    async with engine.connect() as connection:
        return await connection.scalar(
            text("SELECT discarded_through_seq FROM public.market_event_replay_state")
        )


async def _create_disposable_database(admin_url: str, owner_url: str) -> tuple[str, str]:
    name = f"phase6_floor_{uuid4().hex[:12]}_analytics_test"
    owner = make_url(owner_url)
    admin = make_url(admin_url)
    engine = create_async_engine(admin_url, isolation_level="AUTOCOMMIT")
    try:
        async with engine.connect() as connection:
            await connection.execute(
                text(f'CREATE DATABASE "{name}" OWNER "{owner.username}"')
            )
    finally:
        await engine.dispose()
    target_admin = create_async_engine(
        str(admin.set(database=name).render_as_string(hide_password=False)),
        isolation_level="AUTOCOMMIT",
    )
    try:
        async with target_admin.connect() as connection:
            await connection.execute(text("CREATE EXTENSION IF NOT EXISTS postgis"))
    finally:
        await target_admin.dispose()
    return name, owner.set(database=name).render_as_string(hide_password=False)


async def _drop_disposable_database(admin_url: str, name: str) -> None:
    admin = create_async_engine(admin_url, isolation_level="AUTOCOMMIT")
    try:
        async with admin.connect() as connection:
            await connection.execute(text(f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)'))
    finally:
        await admin.dispose()


def _alembic(database_url: str, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-m", "alembic", *arguments],
        cwd=ROOT,
        env={
            "PATH": os.environ.get("PATH", ""),
            "DATABASE_URL": database_url,
            "MIGRATOR_DATABASE_URL": database_url,
            "ENVIRONMENT": "test",
            "RELEASE_SHA": "test",
            "JWT_SECRET": "test-secret-key-that-is-at-least-32-characters-long",
            "BACKEND_CORS_ORIGINS": "[]",
        },
        text=True,
        capture_output=True,
        check=False,
    )


def _run_store_test(database_url: str, name: str) -> None:
    completed = subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            "rs/Cargo.toml",
            "--locked",
            "-p",
            "verdaxis-read",
            "--features",
            "contract-tests",
            "--test",
            "streams_store",
            name,
            "--",
            "--exact",
            "--nocapture",
        ],
        cwd=ROOT,
        env={
            **os.environ,
            "STREAMS_STORE_TEST_DATABASE_URL": database_url.replace(
                "postgresql+asyncpg://", "postgresql://"
            ),
        },
        text=True,
        capture_output=True,
        check=False,
        timeout=900,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr


def _role_policy(database_url: str, script: str) -> subprocess.CompletedProcess[str]:
    url = make_url(database_url)
    admin = make_url(_require("POSTGRES_ADMIN_TEST_DATABASE_URL"))
    command = [
        "psql", "-X", "-v", "ON_ERROR_STOP=1",
        "-h", admin.host, "-p", str(admin.port), "-U", admin.username,
        "-d", url.database,
        "-v", f"database_name={url.database}",
        "-v", f"migrator_role={url.username}",
        "-v", f"app_role={_require('RUNTIME_TEST_APP_ROLE')}",
        "-v", f"backup_role={_require('RUNTIME_TEST_BACKUP_ROLE')}",
        "-f", str(ROOT / "deploy/postgres" / script),
    ]
    return subprocess.run(
        command, env={**os.environ, "PGPASSWORD": admin.password},
        text=True, capture_output=True, check=False, timeout=60,
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("revision", [PARENT_REVISION, REVISION])
async def test_retention_role_policy_at_checkpoint(retention_urls, revision):
    name, database_url = await _create_disposable_database(
        retention_urls["admin"], retention_urls["migrator"]
    )
    try:
        upgraded = _alembic(database_url, "upgrade", revision)
        assert upgraded.returncode == 0, upgraded.stderr
        converged = _role_policy(database_url, "bootstrap_roles.sql")
        assert converged.returncode == 0, converged.stderr
        validated = _role_policy(database_url, "validate_roles.sql")
        assert validated.returncode == 0, validated.stderr
    finally:
        await _drop_disposable_database(retention_urls["admin"], name)


async def _validate_after_catalog_change(retention_urls, revision: str, statement: str):
    """Validate role policy after an admin catalog change at one checkpoint."""

    # Isolate committed catalog mutations from the shared suite database.
    name, database_url = await _create_disposable_database(
        retention_urls["admin"], retention_urls["migrator"]
    )
    try:
        upgraded = _alembic(database_url, "upgrade", revision)
        assert upgraded.returncode == 0, upgraded.stderr
        converged = _role_policy(database_url, "bootstrap_roles.sql")
        assert converged.returncode == 0, converged.stderr
        admin_url = make_url(retention_urls["admin"]).set(database=name)
        engine = create_async_engine(admin_url)
        try:
            async with engine.begin() as connection:
                for command in statement.split("; "):
                    await connection.execute(text(command))
        finally:
            await engine.dispose()
        return _role_policy(database_url, "validate_roles.sql")
    finally:
        await _drop_disposable_database(retention_urls["admin"], name)


@pytest.mark.asyncio
@pytest.mark.parametrize("statement", [
    "DROP TRIGGER trg_market_event_outbox_replay_floor ON public.market_event_outbox; "
    "CREATE TRIGGER trg_market_event_outbox_replay_floor AFTER DELETE "
    "ON public.market_event_outbox FOR EACH ROW "
    "EXECUTE FUNCTION public.market_event_outbox_advance_replay_floor(); "
    "ALTER TABLE public.market_event_outbox ENABLE ALWAYS TRIGGER "
    "trg_market_event_outbox_replay_floor",
    "DROP TRIGGER trg_market_event_outbox_replay_floor ON public.market_event_outbox; "
    "CREATE TRIGGER trg_market_event_outbox_replay_floor BEFORE DELETE "
    "ON public.market_event_outbox FOR EACH ROW WHEN (OLD.stream_seq IS NULL) "
    "EXECUTE FUNCTION public.market_event_outbox_advance_replay_floor(); "
    "ALTER TABLE public.market_event_outbox ENABLE ALWAYS TRIGGER "
    "trg_market_event_outbox_replay_floor",
    "DROP TRIGGER trg_market_event_outbox_refuse_truncate ON public.market_event_outbox; "
    "CREATE TRIGGER trg_market_event_outbox_refuse_truncate BEFORE TRUNCATE "
    "ON public.market_event_outbox FOR EACH STATEMENT "
    "EXECUTE FUNCTION public.market_event_outbox_advance_replay_floor(); "
    "ALTER TABLE public.market_event_outbox ENABLE ALWAYS TRIGGER "
    "trg_market_event_outbox_refuse_truncate",
    "ALTER FUNCTION public.market_event_outbox_refuse_truncate() OWNER TO postgres",
])
async def test_retention_role_policy_requires_exact_objects(retention_urls, statement):
    validated = await _validate_after_catalog_change(retention_urls, REVISION, statement)
    assert validated.returncode != 0, "modified retention objects were accepted"
    assert "replay retention" in validated.stderr, validated.stderr


@pytest.mark.asyncio
@pytest.mark.parametrize(("revision", "statement"), [
    (
        PARENT_REVISION,
        "CREATE FUNCTION public.market_event_outbox_refuse_truncate() RETURNS trigger "
        "LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$",
    ),
    (REVISION, "DROP TABLE public.market_event_retention_control"),
])
async def test_retention_role_policy_refuses_partial_objects(retention_urls, revision, statement):
    validated = await _validate_after_catalog_change(retention_urls, revision, statement)
    assert validated.returncode != 0, "a partial retention object set was accepted"
    assert "replay retention" in validated.stderr, validated.stderr


def test_store_driver_requires_a_selected_test(retention_urls):
    with pytest.raises(AssertionError, match="running 0 tests"):
        _run_store_test(retention_urls["app"], "postgres_no_such_store_case")


@pytest.fixture(scope="module")
def retention_urls(analytics_pg_url):
    urls = {
        "migrator": _require("MIGRATOR_DATABASE_URL"),
        "app": _require("DATABASE_URL"),
        "backup": _require("BACKUP_DATABASE_URL"),
        "admin": _require("POSTGRES_ADMIN_TEST_DATABASE_URL"),
    }
    expected = make_url(analytics_pg_url)
    for role, url in urls.items():
        actual = make_url(url)
        assert (actual.host, actual.port, actual.database) == (
            expected.host,
            expected.port,
            expected.database,
        ), f"{role} URL must address the disposable analytics database"
    if not expected.database.endswith("_analytics_test"):
        raise RuntimeError("retention proofs require a *_analytics_test database")
    return urls


@pytest.fixture
async def owner_engine(retention_urls):
    engine = create_async_engine(retention_urls["migrator"], hide_parameters=True)
    try:
        async with engine.begin() as connection:
            await record_ownership(connection)
            await clear_retention(connection)
        yield engine
    finally:
        await engine.dispose()


def test_migration_has_one_head_and_direct_parent():
    script = ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))

    assert script.get_heads() == [REVISION]
    revision = script.get_revision(REVISION)
    assert revision is not None
    assert revision.down_revision == PARENT_REVISION
    assert script.get_revision(PARENT_REVISION) is not None
    policy = (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
    assert f"{PARENT_REVISION}\t{REVISION}" in policy
    assert f"{REVISION}\t{REVISION}" in policy


@pytest.mark.asyncio
async def test_retention_bootstrap_fence(retention_urls):
    name, database_url = await _create_disposable_database(
        retention_urls["admin"], retention_urls["migrator"]
    )
    engine = create_async_engine(database_url)
    try:
        assert _alembic(database_url, "upgrade", PARENT_REVISION).returncode == 0
        async with engine.connect() as blocker:
            transaction = await blocker.begin()
            await blocker.execute(
                text("LOCK TABLE public.market_event_outbox IN ROW EXCLUSIVE MODE")
            )
            started = time.monotonic()
            completed = await asyncio.to_thread(_alembic, database_url, "upgrade", REVISION)
            elapsed = time.monotonic() - started
            await transaction.rollback()
        output = (completed.stdout + completed.stderr).lower()
        assert completed.returncode != 0, completed.stdout
        assert "lock" in output, output
        assert elapsed < 60, "the bootstrap fence must refuse instead of waiting"
    finally:
        await engine.dispose()
        await _drop_disposable_database(retention_urls["admin"], name)


@pytest.mark.asyncio
async def test_retention_delete_rollback(owner_engine):
    await _insert_event(owner_engine, seq=7, participants=[ORG])

    async with owner_engine.connect() as connection:
        transaction = await connection.begin()
        await connection.execute(
            text("DELETE FROM public.market_event_outbox WHERE stream_seq = 7")
        )
        assert await connection.scalar(
            text("SELECT discarded_through_seq FROM public.market_event_replay_state")
        ) == 7
        await transaction.rollback()

    async with owner_engine.connect() as connection:
        assert await connection.scalar(
            text("SELECT count(*) FROM public.market_event_outbox")
        ) == 1
    assert await _floor(owner_engine) == 0


@pytest.mark.asyncio
async def test_retention_deleted_maximum(owner_engine):
    for seq in (10, 20, 30):
        await _insert_event(owner_engine, seq=seq, participants=[ORG])

    async with owner_engine.begin() as connection:
        await connection.execute(
            text("DELETE FROM public.market_event_outbox WHERE stream_seq IN (10, 30)")
        )
    assert await _floor(owner_engine) == 30

    async with owner_engine.begin() as connection:
        await connection.execute(
            text("DELETE FROM public.market_event_outbox WHERE stream_seq = 20")
        )
    assert await _floor(owner_engine) == 30


@pytest.mark.asyncio
async def test_retention_concurrent_delete(owner_engine):
    await _insert_event(owner_engine, seq=5, participants=[ORG])
    await _insert_event(owner_engine, seq=9, participants=[ORG])

    async with owner_engine.connect() as first:
        transaction = await first.begin()
        try:
            await first.execute(
                text("DELETE FROM public.market_event_outbox WHERE stream_seq = 5")
            )
            async with owner_engine.connect() as second:
                second_pid = await second.scalar(text("SELECT pg_backend_pid()"))
                first_pid = await first.scalar(text("SELECT pg_backend_pid()"))
                pending = asyncio.create_task(second.execute(
                    text("DELETE FROM public.market_event_outbox WHERE stream_seq = 9")
                ))
                try:
                    async def wait_for_blocked_delete():
                        async with owner_engine.connect() as observer:
                            while not await observer.scalar(
                                text("SELECT :first = ANY(pg_blocking_pids(:second))"),
                                {"first": first_pid, "second": second_pid},
                            ):
                                assert not pending.done(), "second delete did not wait on the floor"
                                await asyncio.sleep(0.01)

                    await asyncio.wait_for(wait_for_blocked_delete(), timeout=10)
                    # First transaction still owns the floor; the second delete
                    # may finish only after this explicit commit barrier.
                    assert await _floor(owner_engine) == 0
                    await transaction.commit()
                    await asyncio.wait_for(pending, timeout=10)
                    await second.commit()
                finally:
                    if not pending.done():
                        pending.cancel()
                    await asyncio.gather(pending, return_exceptions=True)
        finally:
            if transaction.is_active:
                await transaction.rollback()

    assert await _floor(owner_engine) == 9
    async with owner_engine.connect() as connection:
        assert await connection.scalar(
            text("SELECT count(*) FROM public.market_event_outbox")
        ) == 0


@pytest.mark.asyncio
async def test_retention_pending_delete_refused(owner_engine):
    await _insert_event(owner_engine, seq=None, participants=[ORG])

    await _expect_error(
        owner_engine,
        "DELETE FROM public.market_event_outbox WHERE stream_seq IS NULL",
        code="23514",
    )
    async with owner_engine.connect() as connection:
        assert await connection.scalar(
            text("SELECT count(*) FROM public.market_event_outbox")
        ) == 1
    assert await _floor(owner_engine) == 0


@pytest.mark.asyncio
async def test_retention_truncate_refused(owner_engine):
    await _insert_event(owner_engine, seq=3, participants=[ORG])

    await _expect_error(
        owner_engine,
        "TRUNCATE TABLE public.market_event_outbox",
        code="42501",
    )
    async with owner_engine.connect() as connection:
        assert await connection.scalar(
            text("SELECT count(*) FROM public.market_event_outbox")
        ) == 1


@pytest.mark.asyncio
async def test_retention_app_acl(retention_urls):
    engine = create_async_engine(retention_urls["app"], hide_parameters=True)
    try:
        for table in TABLES + ("market_event_outbox",):
            async with engine.connect() as connection:
                await connection.execute(text(f"SELECT 1 FROM public.{table} LIMIT 1"))
        for statement in (
            "INSERT INTO public.market_event_replay_state (singleton) VALUES (true)",
            "UPDATE public.market_event_replay_state SET discarded_through_seq = 1",
            "DELETE FROM public.market_event_replay_state",
            "INSERT INTO public.market_event_retention_control (singleton) VALUES (true)",
            "UPDATE public.market_event_retention_control SET mode = 'blocked'",
            "DELETE FROM public.market_event_retention_control",
            "DELETE FROM public.market_event_outbox WHERE false",
        ):
            await _expect_error(engine, statement, code="42501")
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_retention_backup_read(retention_urls):
    engine = create_async_engine(retention_urls["backup"], hide_parameters=True)
    try:
        for table in TABLES + ("market_event_outbox",):
            async with engine.connect() as connection:
                await connection.scalar(text(f"SELECT count(*) FROM public.{table}"))
        await _expect_error(
            engine,
            "INSERT INTO public.market_event_replay_state (singleton) VALUES (true)",
            code="42501",
        )
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_retention_migration_atomicity(retention_urls):
    name, database_url = await _create_disposable_database(
        retention_urls["admin"], retention_urls["migrator"]
    )
    engine = create_async_engine(database_url)
    try:
        assert _alembic(database_url, "upgrade", PARENT_REVISION).returncode == 0
        await _insert_event(engine, seq=10, participants=[ORG])
        await _insert_event(engine, seq=20, participants=[ORG])

        # The index is created after both tables, functions and triggers. A
        # relation-name conflict makes the migration fail at that final DDL.
        async with engine.begin() as connection:
            await connection.execute(text(
                "CREATE TABLE public.ix_market_event_outbox_participants_gin (id integer)"
            ))
        failed = await asyncio.to_thread(_alembic, database_url, "upgrade", REVISION)
        assert failed.returncode != 0
        assert 'relation "ix_market_event_outbox_participants_gin" already exists' in failed.stderr
        async with engine.connect() as inspector:
            for table in TABLES:
                assert await inspector.scalar(
                    text("SELECT to_regclass(:name) IS NULL"), {"name": f"public.{table}"}
                )
            assert await inspector.scalar(
                text("SELECT count(*) FROM pg_catalog.pg_proc WHERE proname = ANY(:names)"),
                {"names": list(FUNCTIONS)},
            ) == 0
            assert await inspector.scalar(
                text("SELECT count(*) FROM pg_catalog.pg_trigger WHERE tgname = ANY(:names)"),
                {"names": [
                    "trg_market_event_outbox_replay_floor",
                    "trg_market_event_outbox_refuse_truncate",
                ]},
            ) == 0
            assert await inspector.scalar(text("SELECT version_num FROM alembic_version")) == PARENT_REVISION
        async with engine.begin() as connection:
            await connection.execute(text("DROP TABLE public.ix_market_event_outbox_participants_gin"))

        upgraded = _alembic(database_url, "upgrade", REVISION)
        assert upgraded.returncode == 0, upgraded.stderr
        async with engine.connect() as inspector:
            assert await inspector.scalar(
                text(
                    "SELECT discarded_through_seq FROM public.market_event_replay_state"
                )
            ) == 20
            assert await inspector.scalar(
                text(
                    "SELECT count(*) FROM pg_catalog.pg_trigger "
                    "WHERE tgname = ANY(:names)"
                ),
                {"names": [
                    "trg_market_event_outbox_replay_floor",
                    "trg_market_event_outbox_refuse_truncate",
                ]},
            ) == 2

        refused = _alembic(database_url, "downgrade", "-1")
        assert refused.returncode != 0
        async with engine.connect() as inspector:
            assert await inspector.scalar(
                text("SELECT to_regclass('public.market_event_replay_state') IS NOT NULL")
            )
            assert await inspector.scalar(
                text("SELECT discarded_through_seq FROM public.market_event_replay_state")
            ) == 20
    finally:
        await engine.dispose()
        await _drop_disposable_database(retention_urls["admin"], name)


@pytest.mark.asyncio
async def test_retention_empty_store(owner_engine, retention_urls):
    _run_store_test(retention_urls["app"], "postgres_empty_store_boundary")


@pytest.mark.asyncio
async def test_retention_sequence_holes(owner_engine, retention_urls):
    await _insert_event(
        owner_engine,
        seq=10,
        participants=[ORG],
        payload={"schema_version": 1, "_public_market_invalidation": True},
    )
    await _insert_event(owner_engine, seq=20, participants=[FOREIGN_ORG])
    await _insert_event(owner_engine, seq=30, participants=[ORG])

    _run_store_test(retention_urls["app"], "postgres_sequence_holes_and_scope")


@pytest.mark.asyncio
async def test_replay_state_missing_is_unavailable(owner_engine, retention_urls):
    async with owner_engine.begin() as connection:
        await connection.execute(text("DELETE FROM public.market_event_replay_state"))

    _run_store_test(retention_urls["app"], "postgres_missing_state_is_unavailable")
    await _insert_event(owner_engine, seq=7, participants=[ORG])
    await _expect_error(
        owner_engine,
        "DELETE FROM public.market_event_outbox WHERE stream_seq = 7",
        code="55000",
    )
    async with owner_engine.connect() as connection:
        assert await connection.scalar(
            text("SELECT count(*) FROM public.market_event_outbox WHERE stream_seq = 7")
        ) == 1


@pytest.mark.asyncio
async def test_replay_state_singleton_constraint(owner_engine):
    await _expect_error(
        owner_engine,
        "INSERT INTO public.market_event_replay_state (singleton, discarded_through_seq) "
        "VALUES (true, 0)",
        code="23505",
    )
    await _expect_error(
        owner_engine,
        "INSERT INTO public.market_event_replay_state (singleton, discarded_through_seq) "
        "VALUES (false, 0)",
        code="23514",
    )


@pytest.mark.asyncio
async def test_replay_state_negative_floor_refused(owner_engine):
    await _expect_error(
        owner_engine,
        "UPDATE public.market_event_replay_state SET discarded_through_seq = -1",
        code="23514",
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("role", ["app", "backup"])
async def test_non_owner_cannot_replace_retention_objects(retention_urls, role):
    engine = create_async_engine(retention_urls[role], hide_parameters=True)
    try:
        for statement in (
            "CREATE OR REPLACE FUNCTION public.market_event_outbox_advance_replay_floor() "
            "RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN OLD; END $$",
            "DROP TRIGGER trg_market_event_outbox_replay_floor "
            "ON public.market_event_outbox",
            "ALTER TABLE public.market_event_outbox "
            "DISABLE TRIGGER trg_market_event_outbox_replay_floor",
            "CREATE TABLE public.market_event_replay_state_shadow (id integer)",
            "CREATE SCHEMA retention_shadow",
            "DROP FUNCTION public.market_event_outbox_advance_replay_floor()",
        ):
            await _expect_error(engine, statement, code="42501")
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_retention_search_path_is_fixed(retention_urls):
    engine = create_async_engine(retention_urls["migrator"], hide_parameters=True)
    try:
        async with engine.connect() as connection:
            rows = (
                await connection.execute(
                    text(
                        "SELECT proname, prosecdef, proconfig FROM pg_catalog.pg_proc "
                        "WHERE proname = ANY(:names)"
                    ),
                    {"names": list(FUNCTIONS)},
                )
            ).all()
        assert {row.proname for row in rows} == set(FUNCTIONS)
        for row in rows:
            assert row.prosecdef is True
            settings = {item.split("=", 1)[0]: item.split("=", 1)[1] for item in row.proconfig or []}
            assert settings.get("search_path") == "pg_catalog, public", row.proconfig
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_app_cannot_execute_retention_function(retention_urls):
    engine = create_async_engine(retention_urls["app"], hide_parameters=True)
    try:
        async with engine.connect() as connection:
            for name in FUNCTIONS:
                assert not await connection.scalar(
                    text("SELECT has_function_privilege(current_user, :name, 'EXECUTE')"),
                    {"name": f"public.{name}()"},
                )
        for name in FUNCTIONS:
            await _expect_error(
                engine,
                f"DO $$ BEGIN PERFORM public.{name}(); END $$",
                code="42501",
            )
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_prune_is_refused_while_python_owns_private_streams(owner_engine):
    await _insert_event(owner_engine, seq=11, participants=[ORG])
    async with owner_engine.begin() as connection:
        await record_ownership(connection, mode="blocked")

    await _expect_error(
        owner_engine,
        "DELETE FROM public.market_event_outbox WHERE stream_seq = 11",
        code="42501",
    )
    assert await _floor(owner_engine) == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("ownership", [
    {"drained": False},
    {"environment": None},
    {"environment": ""},
    {"release_sha": None},
    {"selector_digest": None},
])
async def test_prune_is_refused_during_drain(owner_engine, ownership):
    await _insert_event(owner_engine, seq=12, participants=[ORG])
    async with owner_engine.begin() as connection:
        await record_ownership(connection, mode="rust_active", **ownership)

    await _expect_error(
        owner_engine,
        "DELETE FROM public.market_event_outbox WHERE stream_seq = 12",
        code="42501",
    )


@pytest.mark.asyncio
async def test_ownership_block_waits_for_inflight_prune(owner_engine):
    await _insert_event(owner_engine, seq=13, participants=[ORG])

    async with owner_engine.connect() as pruning:
        transaction = await pruning.begin()
        await pruning.execute(
            text("DELETE FROM public.market_event_outbox WHERE stream_seq = 13")
        )
        async with owner_engine.connect() as blocking:
            with pytest.raises(DBAPIError) as caught:
                await blocking.execute(text("SET LOCAL lock_timeout = '200ms'"))
                await blocking.execute(
                    text(
                        "SELECT 1 FROM public.market_event_retention_control "
                        "WHERE singleton FOR UPDATE"
                    )
                )
            assert _sqlstate(caught.value) == "55P03"
        await transaction.commit()

    async with owner_engine.begin() as connection:
        await connection.execute(
            text(
                "SELECT 1 FROM public.market_event_retention_control "
                "WHERE singleton FOR UPDATE"
            )
        )
    assert await _floor(owner_engine) == 13


@pytest.mark.asyncio
async def test_missing_owner_record_refuses_prune(owner_engine):
    await _insert_event(owner_engine, seq=14, participants=[ORG])
    async with owner_engine.begin() as connection:
        await connection.execute(text("DELETE FROM public.market_event_retention_control"))

    await _expect_error(
        owner_engine,
        "DELETE FROM public.market_event_outbox WHERE stream_seq = 14",
        code="42501",
    )
    async with owner_engine.begin() as connection:
        await record_ownership(connection)


@pytest.mark.asyncio
async def test_replay_membership_index_shape(retention_urls):
    engine = create_async_engine(retention_urls["migrator"], hide_parameters=True)
    try:
        async with engine.connect() as connection:
            row = (
                await connection.execute(
                    text(
                        """
                        SELECT am.amname,
                               index.indisvalid,
                               index.indisready,
                               pg_get_expr(index.indexprs, index.indrelid) AS expression,
                               (SELECT opc.opcname FROM pg_catalog.pg_opclass AS opc
                                 WHERE opc.oid = index.indclass[0]) AS opclass
                        FROM pg_catalog.pg_index AS index
                        JOIN pg_catalog.pg_class AS relation
                          ON relation.oid = index.indexrelid
                        JOIN pg_catalog.pg_am AS am ON am.oid = relation.relam
                        WHERE relation.relname = 'ix_market_event_outbox_participants_gin'
                        """
                    )
                )
            ).one_or_none()
        assert row is not None
        assert row.amname == "gin"
        assert row.indisvalid is True
        assert row.indisready is True
        assert row.opclass == "jsonb_ops"
        assert "participant_org_ids" in row.expression
        assert "jsonb" in row.expression
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_sparse_org_replay_index_path(owner_engine, retention_urls):
    async with owner_engine.begin() as connection:
        await connection.execute(
            text(
                "INSERT INTO public.market_event_outbox "
                "(id, event_type, aggregate_type, aggregate_id, participant_org_ids, "
                "payload, created_at, stream_seq) "
                "SELECT gen_random_uuid(), 'trade_created', 'trade', number::text, "
                "json_build_array(CASE WHEN number = 1 THEN :org ELSE :foreign END), "
                "json_build_object('schema_version', 1), now(), number "
                "FROM generate_series(1, 2000) AS number"
            ),
            {"org": str(ORG), "foreign": str(FOREIGN_ORG)},
        )
        await connection.execute(text("ANALYZE public.market_event_outbox"))

    engine = create_async_engine(retention_urls["migrator"], hide_parameters=True)
    try:
        async with engine.connect() as connection:
            # Deterministic test-only planner settings: forbid the sequential
            # scan and the ordered unique-index scan so only the membership
            # index can satisfy the predicate and ordering.
            await connection.execute(text("SET enable_seqscan = off"))
            await connection.execute(text("SET enable_indexscan = off"))
            await connection.execute(text("SET enable_bitmapscan = on"))
            plan = await connection.scalar(
                text(
                    "EXPLAIN (FORMAT JSON) "
                    "SELECT stream_seq FROM public.market_event_outbox "
                    "WHERE (participant_org_ids::jsonb) ? :org "
                    "AND stream_seq > 0 ORDER BY stream_seq LIMIT 500"
                ),
                {"org": str(ORG)},
            )
            await connection.execute(text("RESET enable_seqscan"))
            await connection.execute(text("RESET enable_indexscan"))
            await connection.execute(text("RESET enable_bitmapscan"))
            default_plan = await connection.scalar(
                text(
                    "EXPLAIN (FORMAT JSON) "
                    "SELECT stream_seq FROM public.market_event_outbox "
                    "WHERE (participant_org_ids::jsonb) ? :org "
                    "AND stream_seq > 0 ORDER BY stream_seq LIMIT 500"
                ),
                {"org": str(ORG)},
            )
            print("default membership plan:", default_plan)
            retained = (
                await connection.execute(
                    text(
                        "SELECT stream_seq FROM public.market_event_outbox "
                        "WHERE (participant_org_ids::jsonb) ? :org "
                        "AND stream_seq > 0 ORDER BY stream_seq LIMIT 500"
                    ),
                    {"org": str(ORG)},
                )
            ).scalars().all()
        rendered = str(plan)
        assert "ix_market_event_outbox_participants_gin" in rendered, rendered
        assert retained == [1]
    finally:
        await engine.dispose()


@pytest.mark.asyncio
async def test_deleted_prefix_advances_floor_atomically(owner_engine, retention_urls):
    for seq in (5, 15, 25):
        await _insert_event(owner_engine, seq=seq, participants=[ORG])

    _run_store_test(retention_urls["app"], "postgres_prefix_retained")

    async with owner_engine.begin() as connection:
        await connection.execute(
            text("DELETE FROM public.market_event_outbox WHERE stream_seq IN (5, 15)")
        )
    assert await _floor(owner_engine) == 15

    _run_store_test(retention_urls["app"], "postgres_prefix_discarded")

    async with owner_engine.begin() as connection:
        await connection.execute(text("DELETE FROM public.market_event_outbox"))
    _run_store_test(retention_urls["app"], "postgres_empty_retained_history_boundary")


@pytest.mark.asyncio
@pytest.mark.parametrize("row_shape", ["large_participant", "event_name"])
async def test_store_rejects_invalid_row_metadata(owner_engine, retention_urls, row_shape):
    identifier = await _insert_event(owner_engine, seq=10, participants=[ORG])
    async with owner_engine.begin() as connection:
        if row_shape == "event_name":
            await connection.execute(
                text("UPDATE market_event_outbox SET event_type = :value WHERE id = :id"),
                {"id": identifier, "value": "trade_created\nforged"},
            )
        else:
            participants = [str(ORG), "x" * 300_000]
            await connection.execute(
                text("UPDATE market_event_outbox SET participant_org_ids = CAST(:value AS json) WHERE id = :id"),
                {"id": identifier, "value": json.dumps(participants)},
            )
    _run_store_test(retention_urls["app"], "postgres_invalid_row_is_classified")


@pytest.mark.asyncio
async def test_store_pool_acquisition_is_bounded(owner_engine, retention_urls):
    _run_store_test(retention_urls["app"], "postgres_pool_acquisition_is_bounded")
