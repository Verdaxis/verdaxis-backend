"""Trade freeze schema, retained history, runtime grants and release checkpoints."""

import ast
import asyncio
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from uuid import uuid4

import pytest
from sqlalchemy.engine import make_url

from tests.postgres.test_supplier_offer_migration_roundtrip import (
    supplier_scratch_database,  # noqa: F401
)

ROOT = Path(__file__).resolve().parents[2]
PARENT = "p89_20261022_watchlist_producer"
REVISION = "p89_20261022_trade_freeze"
MIGRATION = ROOT / f"alembic/versions/{REVISION}.py"
ACTOR = "88888888-9999-0000-0000-000000000001"
BUYER = "88888888-9999-0000-0000-000000000002"
SELLER = "88888888-9999-0000-0000-000000000003"
TRADE = "88888888-9999-0000-0000-000000000004"
UPDATES = {"status", "cleared_at", "cleared_by_user_id", "clear_note", "version"}
REASONS = {
    "PAYMENT_DISPUTE", "DELIVERY_DISPUTE", "TERMS_DISPUTE",
    "COMPLIANCE_REVIEW", "FRAUD_REVIEW", "COURT_ORDER", "OTHER",
}


FREEZE_OBJECTS = {
    ("public", "trade_freezes", "table", "trade_freezes"),
    ("public", "trade_freezes", "column", "id"),
    ("public", "trade_freezes", "column", "trade_id"),
    ("public", "trade_freezes", "column", "status"),
    ("public", "trade_freezes", "column", "reason_code"),
    ("public", "trade_freezes", "column", "note"),
    ("public", "trade_freezes", "column", "frozen_by_user_id"),
    ("public", "trade_freezes", "column", "frozen_at"),
    ("public", "trade_freezes", "column", "cleared_by_user_id"),
    ("public", "trade_freezes", "column", "cleared_at"),
    ("public", "trade_freezes", "column", "clear_note"),
    ("public", "trade_freezes", "column", "version"),
    ("public", "trade_freezes", "index", "uq_trade_freezes_one_active"),
    ("public", "trade_freezes", "index", "ix_trade_freezes_active_frozen_at"),
    ("public", "trade_freezes", "primary_key_constraint", "trade_freezes_pkey"),
    ("public", "trade_freezes", "foreign_key_constraint", "trade_freezes_trade_id_fkey"),
    ("public", "trade_freezes", "foreign_key_constraint", "trade_freezes_frozen_by_user_id_fkey"),
    ("public", "trade_freezes", "foreign_key_constraint", "trade_freezes_cleared_by_user_id_fkey"),
    ("public", "trade_freezes", "check_constraint", "ck_trade_freezes_status"),
    ("public", "trade_freezes", "check_constraint", "ck_trade_freezes_reason"),
    ("public", "trade_freezes", "check_constraint", "ck_trade_freezes_note"),
    ("public", "trade_freezes", "check_constraint", "ck_trade_freezes_clear"),
    ("public", "trade_freezes", "check_constraint", "ck_trade_freezes_version"),
    ("public", "trades", "trigger", "trades_freeze_guard"),
    ("public", "trade_freezes", "trigger", "trade_freezes_history_guard"),
    ("public", "trades_freeze_guard", "function", "trades_freeze_guard"),
    ("public", "trade_freezes_history_guard", "function", "trade_freezes_history_guard"),
}


def test_rust_owned_freeze_declaration_is_exact():
    from app.migration_drift import RUST_OWNED_SCHEMA_OBJECTS
    from tests.unit.test_watchlist_producer_metadata import RUST_OBJECTS

    assert RUST_OWNED_SCHEMA_OBJECTS == RUST_OBJECTS | FREEZE_OBJECTS


@pytest.mark.parametrize("relation", ["trade_freezes", "market_event_outbox"])
def test_unlisted_column_still_reports_schema_drift(freeze_database, relation):
    import sys

    sql, _ = freeze_database
    sql(f"ALTER TABLE {relation} ADD COLUMN unlisted_extra text")
    url = make_url(database_url(sql, "MARKET_INTEGRITY_TEST_DATABASE_URL")).set(
        drivername="postgresql+asyncpg"
    ).render_as_string(hide_password=False)
    result = subprocess.run(
        [sys.executable, "-m", "alembic", "check"], cwd=ROOT,
        env={**os.environ, "DATABASE_URL": url, "MIGRATOR_DATABASE_URL": url},
        capture_output=True, text=True, check=False,
    )
    output = result.stdout + result.stderr
    assert result.returncode != 0, output
    assert "unlisted_extra" in output and relation in output, output
    assert "remove_column" in output or "remove_table" in output, output
    assert "Target database is not up to date" not in output, output


def migration_graph():
    from alembic.config import Config
    from alembic.script import ScriptDirectory

    return ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))


def test_migration_parent_is_the_integrated_head():
    assert MIGRATION.is_file(), "trade freeze revision is absent"
    assignments = {
        node.targets[0].id: ast.literal_eval(node.value)
        for node in ast.parse(MIGRATION.read_text()).body
        if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
    }
    assert assignments["revision"] == REVISION
    assert assignments["down_revision"] == PARENT
    assert migration_graph().get_revision(PARENT).down_revision == "p5_20261020_read_indexes"


@pytest.fixture
def freeze_database(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    assert MIGRATION.is_file(), "trade freeze revision is absent"
    migrate("upgrade", REVISION)
    return sql, migrate


def seed_parties(sql):
    sql(f"""
        INSERT INTO users (id,email,password_hash,role)
        VALUES ('{ACTOR}','freeze-admin@example.test','unused','ADMIN');
        INSERT INTO organizations (id,name,type,provenance) VALUES
          ('{BUYER}','Freeze buyer','FUEL_BUYER','REAL'),
          ('{SELLER}','Freeze seller','FUEL_SUPPLIER','REAL');
    """)


def insert_trade(sql, trade_id):
    sql(f"""
        INSERT INTO trades (id,buyer_id,seller_id,initiator_org_id,initiated_by,
          buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,
          market_product,delivery_point_id,delivery_point_name,delivery_point_region,
          availability_window,quantity_mt,price_per_mt_usd,status,commission_rate_pct)
        SELECT '{trade_id}','{BUYER}','{SELLER}','{BUYER}','BUYER','REAL','REAL',
          p.id,p.name,p.fuel_type,p.fuel_grade,'BIO_METHANOL',d.id,d.name,d.region,
          'SPOT',10,700,'PENDING_CONFIRMATION',0
        FROM products p CROSS JOIN delivery_points d
        WHERE p.name='Bio Methanol' AND d.name='Singapore';
    """)


def seed_trade(sql):
    seed_parties(sql)
    insert_trade(sql, TRADE)
    assert sql("SELECT count(*) FROM trades") == "1"


def freeze_insert(freeze_id=None, trade_id=TRADE):
    return f"""INSERT INTO trade_freezes
        (id,trade_id,status,reason_code,note,frozen_by_user_id)
        VALUES ('{freeze_id or uuid4()}','{trade_id}','ACTIVE','OTHER','Review required','{ACTOR}')"""


def clear_freeze(sql):
    sql(f"""UPDATE trade_freezes SET status='CLEARED',cleared_at=now(),
        cleared_by_user_id='{ACTOR}',clear_note='Review complete',version=version+1""")


def database_url(sql, variable="POSTGRES_ADMIN_TEST_DATABASE_URL"):
    return make_url(os.environ[variable]).set(
        drivername="postgresql", database=sql("SELECT current_database()")
    ).render_as_string(hide_password=False)


def converge(sql):
    app = os.environ["RUNTIME_TEST_APP_ROLE"]
    migrator = os.environ["RUNTIME_TEST_MIGRATOR_ROLE"]
    backup = os.environ["RUNTIME_TEST_BACKUP_ROLE"]
    assert all(re.fullmatch(r"[a-z_][a-z0-9_]*", role) for role in (app, migrator, backup))
    sql(f"ALTER SCHEMA public OWNER TO {migrator}")
    admin = make_url(database_url(sql))
    result = subprocess.run(
        ["psql", "-X", "-h", admin.host, "-p", str(admin.port), "-U", admin.username,
         "-d", admin.database, "--single-transaction", "-v", "ON_ERROR_STOP=1",
         "-v", f"database_name={admin.database}", "-v", f"app_role={app}",
         "-v", f"migrator_role={migrator}", "-v", f"backup_role={backup}",
         "-f", str(ROOT / "deploy/postgres/converge_runtime_object_acls.sql")],
        env={**os.environ, "PGPASSWORD": admin.password},
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return app, migrator


def catalog(sql):
    return {
        "relations": sql("""SELECT relname || ':' || relkind::text FROM pg_class
            WHERE relnamespace='public'::regnamespace ORDER BY 1""").splitlines(),
        "triggers": sql("""SELECT tgrelid::regclass || ':' || tgname || ':' ||
            tgenabled::text || ':' || pg_get_triggerdef(oid) FROM pg_trigger
            WHERE NOT tgisinternal ORDER BY 1""").splitlines(),
        "indexes": sql("""SELECT tablename || ':' || indexname || ':' || indexdef
            FROM pg_indexes WHERE schemaname='public' ORDER BY 1""").splitlines(),
        "constraints": sql("""SELECT conrelid::regclass || ':' || conname || ':' ||
            pg_get_constraintdef(oid) FROM pg_constraint
            WHERE connamespace='public'::regnamespace ORDER BY 1""").splitlines(),
        "functions": sql("""SELECT p.proname || ':' || pg_get_functiondef(p.oid)
            FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace
            WHERE n.nspname='public' AND p.prorettype='trigger'::regtype ORDER BY p.proname"""),
    }


def test_migration_creates_exactly_the_freeze_objects(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    before = catalog(sql)
    migrate("upgrade", REVISION)
    after = catalog(sql)
    for key in ("relations", "triggers", "indexes", "constraints"):
        assert set(before[key]) <= set(after[key]), key
    assert set(after["relations"]) - set(before["relations"]) == {
        "trade_freezes:r", "trade_freezes_pkey:i", "uq_trade_freezes_one_active:i",
        "ix_trade_freezes_active_frozen_at:i",
    }
    new_triggers = set(after["triggers"]) - set(before["triggers"])
    assert len(new_triggers) == 2
    assert any(row.startswith("trades:trades_freeze_guard:O:") for row in new_triggers)
    assert any(row.startswith("trade_freezes:trade_freezes_history_guard:O:") for row in new_triggers)
    assert "BEFORE UPDATE OF status ON public.trades" in "\n".join(new_triggers)
    index_rows = sql("SELECT indexname || ':' || indexdef FROM pg_indexes WHERE tablename='trade_freezes'")
    assert "uq_trade_freezes_one_active:CREATE UNIQUE INDEX" in index_rows
    assert "(frozen_at DESC, id DESC)" in index_rows
    assert index_rows.count("WHERE ((status)::text = 'ACTIVE'::text)") == 2
    assert sql("""SELECT count(*) FROM pg_constraint WHERE conrelid='trade_freezes'::regclass
        AND contype='f' AND confdeltype='r'""") == "3"
    checks = sql("""SELECT conname FROM pg_constraint WHERE conrelid='trade_freezes'::regclass
        AND contype='c' ORDER BY conname""")
    assert set(checks.splitlines()) == {
        "ck_trade_freezes_status", "ck_trade_freezes_reason", "ck_trade_freezes_note",
        "ck_trade_freezes_clear", "ck_trade_freezes_version",
    }
    columns = json.loads(sql("""SELECT json_object_agg(column_name,
        json_build_array(data_type,character_maximum_length,is_nullable,column_default))
        FROM information_schema.columns WHERE table_schema='public' AND table_name='trade_freezes'"""))
    assert set(columns) == {
        "id", "trade_id", "status", "reason_code", "note", "frozen_by_user_id", "frozen_at",
        "cleared_by_user_id", "cleared_at", "clear_note", "version",
    }
    for name in ("id", "trade_id", "frozen_by_user_id", "cleared_by_user_id"):
        assert columns[name][0] == "uuid"
    for name, length in (("status", 8), ("reason_code", 32), ("note", 500), ("clear_note", 500)):
        assert columns[name][:2] == ["character varying", length]
    assert columns["frozen_at"] == ["timestamp with time zone", None, "NO", "now()"]
    assert columns["cleared_at"] == ["timestamp with time zone", None, "YES", None]
    assert columns["version"] == ["integer", None, "NO", "1"]
    for name, details in columns.items():
        assert details[2] == ("YES" if name in {"cleared_at", "cleared_by_user_id", "clear_note"} else "NO")
    assert after["functions"].count("CREATE OR REPLACE FUNCTION public.trades_freeze_guard()") == 1
    assert after["functions"].count("CREATE OR REPLACE FUNCTION public.trade_freezes_history_guard()") == 1
    print("FREEZE_CATALOG=" + json.dumps(after, sort_keys=True))
    migrate("downgrade", PARENT)
    assert catalog(sql) == before
    migrate("upgrade", REVISION)


def test_alembic_heads_is_one_before_and_after(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    graph = migration_graph()
    assert graph.get_heads() == [REVISION]
    parents = {
        parent
        for item in graph.walk_revisions() if item.revision != REVISION
        for parent in (item.down_revision if isinstance(item.down_revision, tuple) else (item.down_revision,))
    }
    previous = {item.revision for item in graph.walk_revisions() if item.revision != REVISION} - parents
    assert previous == {PARENT}
    migrate("upgrade", PARENT)
    assert sql("SELECT version_num FROM alembic_version") == PARENT
    migrate("upgrade", REVISION)
    assert sql("SELECT version_num FROM alembic_version") == REVISION
    migrate("downgrade", PARENT)
    assert sql("SELECT version_num FROM alembic_version") == PARENT
    assert graph.get_heads() == [REVISION]


def test_downgrade_refuses_when_rows_exist(freeze_database):
    sql, migrate = freeze_database
    seed_trade(sql)
    sql(freeze_insert())
    for cleared in (False, True):
        if cleared:
            clear_freeze(sql)
        before = catalog(sql)
        output = migrate("downgrade", PARENT, success=False)
        assert "Retained trade freeze history prevents downgrade" in output
        assert sql("SELECT version_num FROM alembic_version") == REVISION
        assert sql("SELECT count(*) FROM trade_freezes") == "1"
        assert catalog(sql) == before


def test_acl_policy_declares_only_select_insert_and_five_updates(freeze_database):
    sql, _ = freeze_database
    seed_trade(sql)
    app, migrator = converge(sql)
    assert sql(f"SELECT has_table_privilege('{app}','trade_freezes','SELECT,INSERT')") == "t"
    for privilege in ("UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"):
        assert sql(f"SELECT has_table_privilege('{app}','trade_freezes','{privilege}')") == "f"
    mutable = sql(f"""SELECT attname FROM pg_attribute WHERE attrelid='trade_freezes'::regclass
        AND attnum>0 AND NOT attisdropped AND has_column_privilege('{app}',attrelid,attname,'UPDATE')""")
    assert set(mutable.splitlines()) == UPDATES
    for function in ("trades_freeze_guard", "trade_freezes_history_guard"):
        assert sql(f"SELECT pg_get_userbyid(proowner) FROM pg_proc WHERE oid='{function}()'::regprocedure") == migrator
        assert sql(f"SELECT has_function_privilege('{app}','{function}()','EXECUTE')") == "f"
        with pytest.raises(AssertionError, match="permission denied for function"):
            sql(f"SET ROLE {app}; SELECT {function}()")
    sql(f"SET ROLE {app}; " + freeze_insert())
    for column, value in {
        "id": "gen_random_uuid()", "trade_id": f"'{uuid4()}'", "reason_code": "'COURT_ORDER'",
        "note": "'Changed history'", "frozen_by_user_id": f"'{uuid4()}'", "frozen_at": "now()",
    }.items():
        with pytest.raises(AssertionError, match="permission denied"):
            sql(f"SET ROLE {app}; UPDATE trade_freezes SET {column}={value}")
    for operation in ("DELETE FROM trade_freezes", "TRUNCATE trade_freezes"):
        with pytest.raises(AssertionError, match="permission denied"):
            sql(f"SET ROLE {app}; {operation}")
    clear_freeze(lambda statement: sql(f"SET ROLE {app}; {statement}"))
    assert sql(f"SET ROLE {app}; SELECT status FROM trade_freezes").splitlines()[-1] == "CLEARED"
    sql("ALTER TABLE trade_freezes ADD COLUMN future_attribute text")
    assert sql(f"SELECT has_column_privilege('{app}','trade_freezes','future_attribute','UPDATE')") == "f"
    assert sql(f"SELECT has_table_privilege('{app}','watchlist_producer_state','SELECT,INSERT,UPDATE')") == "t"
    assert sql(f"SELECT has_table_privilege('{app}','market_event_outbox','SELECT,INSERT,UPDATE')") == "t"
    for table in ("watchlist_producer_state", "market_event_outbox"):
        assert sql(f"SELECT has_table_privilege('{app}','{table}','DELETE')") == "f"


def test_preparer_accepts_reviewed_production_checkpoints(tmp_path):
    from tests.unit.test_rust_release import _load_preparer

    preparer = _load_preparer()
    release = "a" * 40
    environment = tmp_path / ".env"
    environment.write_text(
        f"ENVIRONMENT=production\nRELEASE_SHA={release}\n"
        "DATABASE_URL=postgresql+asyncpg://verdaxis_app:fixture-secret@127.0.0.1:5432/verdaxis_prod\n"
        "JWT_SECRET=" + "k" * 40 + "\n"
        "JWT_ISSUER=verdaxis-prod-api\nJWT_AUDIENCE=verdaxis-prod-web\n"
        "FRONTEND_URL=https://app.verdaxis.exchange\n"
        "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\nRESEND_API_KEY=re_fixture_key\n"
    )
    policy = {"environment": "production", "db_session_time_zone": "UTC"}
    assert preparer.project_runtime_environment(environment, release, REVISION, policy)["MIGRATION_REVISION"] == REVISION
    for revision in (PARENT, "p0_20261016_union", "p5_20261020_read_indexes"):
        assert preparer.project_runtime_environment(environment, release, revision, policy)["MIGRATION_REVISION"] == revision
    for revision in ("kycg_20261014_staging", "prc_20261018_cache", "*", "head", "unlisted_revision"):
        with pytest.raises(preparer.RustRuntimeError):
            preparer.project_runtime_environment(environment, release, revision, policy)


def test_release_pin_lists_the_new_head():
    revisions = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert revisions == ["kycg_20261014_staging", "p0_20261016_union", "prc_20261018_cache", "prc_20261026_changes", "p3_20261020_auth_tokens", "sse_20261021_replay_floor", "p5_20261020_read_indexes", PARENT, REVISION]
    checkpoints = {tuple(line.split()) for line in (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
                   if line and not line.startswith("#")}
    assert (PARENT, REVISION) in checkpoints
    assert (REVISION, REVISION) in checkpoints
    for path in ("rs/read/src/runtime/config.rs", "scripts/prepare_rust_runtime.py"):
        source = (ROOT / path).read_text()
        assert f'"{REVISION}"' in source
        assert f'"{PARENT}"' in source
    artifact = (ROOT / "rs/read/src/runtime/artifact.rs").read_text()
    assert f'"{REVISION}"' in artifact


@pytest.mark.parametrize("status", ["CONFIRMED", "DELIVERED", "PAID", "DECLINED", "CANCELLED"])
def test_direct_status_transition_rolls_back_staged_rows(freeze_database, status):
    import asyncpg

    sql, _ = freeze_database
    seed_trade(sql)
    app, _ = converge(sql)
    sql(freeze_insert())

    async def attempt():
        connection = await asyncpg.connect(database_url(sql, "RUNTIME_TEST_APP_DATABASE_URL"))
        try:
            assert await connection.fetchval("SELECT current_user") == app
            with pytest.raises(asyncpg.PostgresError) as refusal:
                async with connection.transaction():
                    await connection.execute(f"""INSERT INTO audit_logs (id,action,resource_type,resource_id)
                        VALUES ('{uuid4()}','TRADE_FROZEN','trade','{TRADE}')""")
                    await connection.execute(f"""INSERT INTO market_event_outbox
                        (id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload)
                        VALUES ('{uuid4()}','trade.updated','trade','{TRADE}','["{BUYER}"]','{{}}')""")
                    await connection.execute(f"UPDATE trades SET status='{status}' WHERE id='{TRADE}'")
            assert refusal.value.sqlstate == "ZF001"
            assert await connection.fetchval("SELECT count(*) FROM audit_logs") == 0
            assert await connection.fetchval("SELECT count(*) FROM market_event_outbox") == 0
            assert await connection.fetchval("SELECT status FROM trades") == "PENDING_CONFIRMATION"
            assert await connection.fetchval("SELECT status FROM trade_freezes") == "ACTIVE"
            await connection.execute("UPDATE trades SET status=status")
        finally:
            await connection.close()

    asyncio.run(attempt())


def test_history_fields_and_cleared_rows_remain_immutable(freeze_database):
    import asyncpg

    sql, _ = freeze_database
    seed_trade(sql)
    sql(freeze_insert())

    async def attempt():
        connection = await asyncpg.connect(database_url(sql))
        try:
            for column, value in {
                "id": "gen_random_uuid()", "trade_id": "gen_random_uuid()", "reason_code": "'COURT_ORDER'",
                "note": "'Changed history'", "frozen_by_user_id": "gen_random_uuid()",
                "frozen_at": "frozen_at + interval '1 second'",
            }.items():
                with pytest.raises(asyncpg.PostgresError) as refusal:
                    await connection.execute(f"UPDATE trade_freezes SET {column}={value}")
                assert refusal.value.sqlstate == "ZF002", column
            for assignment in ("version=version+1", "cleared_at=now()", "status='CLEARED'"):
                with pytest.raises(asyncpg.PostgresError):
                    await connection.execute(f"UPDATE trade_freezes SET {assignment}")
            clear_freeze(sql)
            for assignment in ("status='ACTIVE'", "clear_note='Changed clearance'", "version=version+1"):
                with pytest.raises(asyncpg.PostgresError) as refusal:
                    await connection.execute(f"UPDATE trade_freezes SET {assignment}")
                assert refusal.value.sqlstate == "ZF003"
            await connection.execute(freeze_insert())
            assert await connection.fetchval("SELECT count(*) FROM trade_freezes") == 2
            assert await connection.fetchval("SELECT count(*) FROM trade_freezes WHERE status='ACTIVE'") == 1
        finally:
            await connection.close()

    asyncio.run(attempt())


def test_independent_freeze_inserts_have_one_active_winner(freeze_database):
    import asyncpg

    sql, _ = freeze_database
    seed_trade(sql)
    app, _ = converge(sql)

    async def race():
        first = await asyncpg.connect(database_url(sql, "RUNTIME_TEST_APP_DATABASE_URL"))
        second = await asyncpg.connect(database_url(sql, "RUNTIME_TEST_APP_DATABASE_URL"))
        observer = await asyncpg.connect(database_url(sql))
        pending = None
        try:
            assert await first.fetchval("SELECT current_user") == app
            assert await second.fetchval("SELECT pg_backend_pid()") != await first.fetchval("SELECT pg_backend_pid()")
            await first.execute("BEGIN")
            await first.execute(freeze_insert())
            pid = await second.fetchval("SELECT pg_backend_pid()")
            pending = asyncio.create_task(second.execute(freeze_insert()))
            for _ in range(500):
                if await observer.fetchval("SELECT wait_event_type='Lock' FROM pg_stat_activity WHERE pid=$1", pid):
                    break
                await asyncio.sleep(0.01)
            else:
                pytest.fail("second freeze insert did not wait on the uncommitted first insert")
            await first.execute("COMMIT")
            with pytest.raises(asyncpg.UniqueViolationError) as refusal:
                await asyncio.wait_for(pending, 5)
            assert refusal.value.constraint_name == "uq_trade_freezes_one_active"
            assert await observer.fetchval("SELECT count(*) FROM trade_freezes WHERE status='ACTIVE'") == 1
        finally:
            await first.close()
            await second.close()
            await observer.close()
            if pending is not None and not pending.done():
                pending.cancel()
                await asyncio.gather(pending, return_exceptions=True)

    asyncio.run(race())


def test_freeze_checks_and_restrict_ownership(freeze_database):
    sql, _ = freeze_database
    seed_trade(sql)
    for reason in sorted(REASONS):
        sql(freeze_insert().replace("'OTHER'", f"'{reason}'"))
        clear_freeze(lambda statement: sql(statement + " WHERE status='ACTIVE'"))
    for before, after, constraint in (
        ("'ACTIVE'", "'PAUSED'", "ck_trade_freezes_(status|clear)"),
        ("'OTHER'", "'UNKNOWN'", "ck_trade_freezes_reason"),
        ("'Review required'", "'  a  '", "ck_trade_freezes_note"),
    ):
        with pytest.raises(AssertionError, match=constraint):
            sql(freeze_insert().replace(before, after))
    for columns, values in (
        (",version", ",0"), (",cleared_at", ",now()"),
        (",cleared_by_user_id", f",'{ACTOR}'"), (",clear_note", ",'Partial clearance'"),
    ):
        statement = freeze_insert().replace("frozen_by_user_id)", f"frozen_by_user_id{columns})")
        statement = statement.replace(f"'{ACTOR}')", f"'{ACTOR}'{values})")
        with pytest.raises(AssertionError, match="ck_trade_freezes_"):
            sql(statement)
    for operation in (f"DELETE FROM users WHERE id='{ACTOR}'", f"DELETE FROM trades WHERE id='{TRADE}'"):
        with pytest.raises(AssertionError, match="foreign key constraint"):
            sql(operation)


# Release checkpoint authority against a disposable cluster that carries the
# deployed staging identity names, so the real checkpoint, ACL convergence and
# revision-verification helpers run unchanged with their identity checks intact.

READ_INDEXES = "p5_20261020_read_indexes"
STAGING_DATABASE = "verdaxis_staging"
STAGING_APP = "verdaxis_app_staging"
STAGING_MIGRATOR = "verdaxis_migrator_staging"
STAGING_BACKUP = "verdaxis_backup_staging"
STAGING_ROLES = (STAGING_APP, STAGING_MIGRATOR, STAGING_BACKUP)
DEPLOYED_DATABASES = {STAGING_DATABASE, "verdaxis_prod"}
DEPLOYED_ROLES = {*STAGING_ROLES, "verdaxis_app", "verdaxis_migrator", "verdaxis_backup"}
LOOPBACK_HOSTS = {"127.0.0.1", "::1", "localhost"}
RELEASE_ALLOWLIST = [
    "kycg_20261014_staging", "p0_20261016_union", "prc_20261018_cache",
    "prc_20261026_changes", "p3_20261020_auth_tokens", "sse_20261021_replay_floor",
    READ_INDEXES, PARENT, REVISION,
]
PRODUCTION_CHECKPOINTS = {
    "p0_20261016_union", "prc_20261026_changes", "p3_20261020_auth_tokens",
    "sse_20261021_replay_floor", READ_INDEXES, PARENT, REVISION,
}
NON_LITERAL_REVISIONS = ("*", "head", "heads", "", "unlisted_revision", f"{REVISION}_next")
FREEZE_FUNCTIONS = {"trades_freeze_guard", "trade_freezes_history_guard"}
NON_OWNER_TABLE_ACL = """
    SELECT coalesce(grantee.rolname, 'PUBLIC') || ':' || acl.privilege_type
    FROM pg_class AS relation
    CROSS JOIN LATERAL aclexplode(coalesce(relation.relacl, acldefault('r', relation.relowner))) AS acl
    LEFT JOIN pg_roles AS grantee ON grantee.oid = acl.grantee
    WHERE relation.oid = 'public.{table}'::regclass AND acl.grantee <> relation.relowner
    ORDER BY 1"""
COLUMN_ACL = """
    SELECT coalesce(grantee.rolname, 'PUBLIC') || ':' || acl.privilege_type || ':' || attribute.attname
    FROM pg_attribute AS attribute
    CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl
    LEFT JOIN pg_roles AS grantee ON grantee.oid = acl.grantee
    WHERE attribute.attrelid = 'public.{table}'::regclass
      AND attribute.attnum > 0 AND NOT attribute.attisdropped
    ORDER BY 1"""
FUNCTION_ACL = """
    SELECT coalesce(grantee.rolname, 'PUBLIC') || ':' || acl.privilege_type
    FROM pg_proc AS routine
    CROSS JOIN LATERAL aclexplode(coalesce(routine.proacl, acldefault('f', routine.proowner))) AS acl
    LEFT JOIN pg_roles AS grantee ON grantee.oid = acl.grantee
    WHERE routine.oid = 'public.{function}()'::regprocedure
    ORDER BY 1"""
NON_EXTENSION_FUNCTIONS = """
    SELECT routine.proname FROM pg_proc AS routine
    JOIN pg_namespace AS namespace ON namespace.oid = routine.pronamespace
    WHERE namespace.nspname = 'public' AND NOT EXISTS (
        SELECT 1 FROM pg_depend AS dependency
        WHERE dependency.classid = 'pg_proc'::regclass
          AND dependency.objid = routine.oid AND dependency.deptype = 'e')
    ORDER BY 1"""


class SyntheticStagingRelease:
    """Apply the two Phase 9 checkpoints with the deployed helpers, then converge ACLs.

    The cluster must be disposable: a loopback endpoint on which no deployed
    database or role name exists before this fixture creates them.
    """

    def __init__(self, admin, workspace: Path):
        self.admin = admin
        self.workspace = workspace
        self.passwords = {role: uuid4().hex for role in STAGING_ROLES}
        self.source = (workspace / "source").resolve()
        self.environment_file = workspace / ".env"
        self.source_sha = ""
        self.calls = []
        self.functions = {}
        self.app_select_before_convergence = {}
        self.convergence_refusal = None
        self.convergence = None
        self.role_validation = None

    def admin_sql(self, statement: str, database: str) -> str:
        result = subprocess.run(
            ["psql", "-X", "-h", self.admin.host or "", "-p", str(self.admin.port or 5432),
             "-U", self.admin.username or "", "-d", database, "-v", "ON_ERROR_STOP=1",
             "-At", "-c", statement],
            env={**os.environ, "PGPASSWORD": self.admin.password or ""},
            capture_output=True, text=True, check=False,
        )
        assert result.returncode == 0, result.stderr
        return result.stdout.strip()

    def sql(self, statement: str) -> str:
        return self.admin_sql(statement, STAGING_DATABASE)

    def cluster_sql(self, statement: str) -> str:
        return self.admin_sql(statement, self.admin.database or "postgres")

    def prove_isolation(self) -> None:
        """Refuse before any write unless this cluster is a disposable test cluster."""
        if (self.admin.host or "").lower() not in LOOPBACK_HOSTS:
            pytest.fail("synthetic staging identities need a loopback disposable cluster")
        assert self.cluster_sql("SELECT rolsuper FROM pg_roles WHERE rolname = current_user") == "t"
        databases = set(self.cluster_sql("SELECT datname FROM pg_database").splitlines())
        roles = set(self.cluster_sql("SELECT rolname FROM pg_roles").splitlines())
        present = sorted((databases & DEPLOYED_DATABASES) | (roles & DEPLOYED_ROLES))
        if present:
            pytest.fail(f"refusing a cluster that already holds deployed identities: {present}")

    def url(self, role: str, *, driver: str = "postgresql") -> str:
        return (f"{driver}://{role}:{self.passwords[role]}@{self.admin.host}:"
                f"{self.admin.port or 5432}/{STAGING_DATABASE}")

    def create(self) -> None:
        self.cluster_sql(f"CREATE DATABASE {STAGING_DATABASE}")
        self.sql("CREATE EXTENSION IF NOT EXISTS postgis")
        result = subprocess.run(
            ["psql", "-X", "-h", self.admin.host or "", "-p", str(self.admin.port or 5432),
             "-U", self.admin.username or "", "-d", STAGING_DATABASE, "-v", "ON_ERROR_STOP=1",
             "-v", f"database_name={STAGING_DATABASE}", "-v", f"app_role={STAGING_APP}",
             "-v", f"migrator_role={STAGING_MIGRATOR}", "-v", f"backup_role={STAGING_BACKUP}",
             "-f", str(ROOT / "deploy/postgres/bootstrap_roles.sql")],
            env={**os.environ, "PGPASSWORD": self.admin.password or ""},
            capture_output=True, text=True, check=False,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        for role, password in self.passwords.items():
            self.cluster_sql(f"ALTER ROLE {role} PASSWORD '{password}'")

    def drop(self) -> None:
        self.cluster_sql(f"DROP DATABASE IF EXISTS {STAGING_DATABASE} WITH (FORCE)")
        for role in STAGING_ROLES:
            self.cluster_sql(f"DROP ROLE IF EXISTS {role}")

    def prepare_source(self) -> None:
        """A committed, clean checkout of the tested commit, as the deploy helpers require."""
        self.source_sha = subprocess.run(
            ["git", "-C", str(ROOT), "rev-parse", "HEAD"],
            capture_output=True, text=True, check=True,
        ).stdout.strip()
        subprocess.run(["git", "clone", "--quiet", "--no-checkout", str(ROOT), str(self.source)], check=True)
        subprocess.run(["git", "-C", str(self.source), "checkout", "--quiet", "--detach", self.source_sha], check=True)
        status = subprocess.run(
            ["git", "-C", str(self.source), "status", "--porcelain", "--untracked-files=all"],
            capture_output=True, text=True, check=True,
        ).stdout
        assert status == "", status
        self.environment_file.write_text(self.environment_text(migrator=STAGING_MIGRATOR))

    def environment_text(self, *, migrator: str) -> str:
        host, port = self.admin.host, self.admin.port or 5432
        return (
            "ENVIRONMENT=staging\n"
            f"RELEASE_SHA={self.source_sha}\n"
            "JWT_SECRET=" + "k" * 40 + "\n"
            "JWT_ISSUER=verdaxis-staging-api\nJWT_AUDIENCE=verdaxis-staging-web\n"
            f"DATABASE_HOST={host}\nDATABASE_PORT={port}\n"
            f"DATABASE_NAME={STAGING_DATABASE}\nDATABASE_USER={STAGING_APP}\n"
            f"DATABASE_PASSWORD={self.passwords[STAGING_APP]}\n"
            f"DATABASE_URL={self.url(STAGING_APP, driver='postgresql+asyncpg')}\n"
            f"MIGRATOR_DATABASE_URL={self.url(migrator, driver='postgresql+asyncpg')}\n"
        )

    def helper_environment(self, release_sha: str | None = None) -> dict[str, str]:
        # The deploy transaction runs helpers under `env -i PATH=...` plus these two keys.
        return {
            "PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1",
            "ENVIRONMENT": "staging", "RELEASE_SHA": release_sha or self.source_sha,
        }

    def revision(self) -> str:
        return self.sql("SELECT string_agg(version_num, ',' ORDER BY version_num) FROM alembic_version")

    def migrate_setup(self, target: str) -> None:
        from dotenv import dotenv_values

        result = subprocess.run(
            [sys.executable, "-m", "alembic", "upgrade", target], cwd=self.source,
            env={**self.helper_environment(), **dotenv_values(self.environment_file)},
            capture_output=True, text=True, check=False,
        )
        assert result.returncode == 0, result.stdout + result.stderr
        assert self.revision() == target

    def checkpoint(self, label, expected, target, *, source_sha=None, approved_sha=None,
                   release_sha=None, environment_file=None, refusal=None):
        before = self.revision()
        source = source_sha or self.source_sha
        result = subprocess.run(
            [sys.executable, str(self.source / "scripts/apply_migration_checkpoint.py"),
             "--source-root", str(self.source),
             "--environment-file", str(environment_file or self.environment_file),
             "--source-sha", source, "--approved-source-sha", approved_sha or source,
             "--expected-current", expected, "--target", target],
            cwd=self.source, env=self.helper_environment(release_sha),
            capture_output=True, text=True, check=False, timeout=900,
        )
        self.calls.append({
            "label": label, "expected": expected, "target": target, "refusal": refusal,
            "returncode": result.returncode, "output": result.stdout + result.stderr,
            "before": before, "after": self.revision(),
        })

    def converge(self, environment: str = "staging"):
        return subprocess.run(
            [sys.executable, str(self.source / "scripts/converge_runtime_acls.py"),
             "--environment", environment, "--environment-file", str(self.environment_file),
             "--bundle-root", str(self.source)],
            cwd=self.source, env={"PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1"},
            capture_output=True, text=True, check=False, timeout=600,
        )

    def validate_roles(self):
        return subprocess.run(
            ["psql", "-X", "-h", self.admin.host or "", "-p", str(self.admin.port or 5432),
             "-U", self.admin.username or "", "-d", STAGING_DATABASE, "-v", "ON_ERROR_STOP=1",
             "-v", f"database_name={STAGING_DATABASE}", "-v", f"app_role={STAGING_APP}",
             "-v", f"migrator_role={STAGING_MIGRATOR}", "-v", f"backup_role={STAGING_BACKUP}",
             "-f", str(self.source / "deploy/postgres/validate_roles.sql")],
            env={**os.environ, "PGPASSWORD": self.admin.password or ""},
            capture_output=True, text=True, check=False,
        )

    def app_can_select(self) -> dict[str, str]:
        return {
            table: self.sql(f"SELECT has_table_privilege('{STAGING_APP}','{table}','SELECT')")
            for table in ("trade_freezes", "watchlist_producer_state")
        }

    def run(self) -> None:
        self.prepare_source()
        self.migrate_setup(READ_INDEXES)
        self.functions[READ_INDEXES] = set(self.sql(NON_EXTENSION_FUNCTIONS).splitlines())

        # Refusals at the read-index checkpoint: each must leave the revision unchanged.
        self.checkpoint("approval differs", READ_INDEXES, PARENT, approved_sha="0" * 40,
                        refusal="migration approval source SHA is not exact")
        self.checkpoint("runtime release differs", READ_INDEXES, PARENT, release_sha="1" * 40,
                        refusal="effective runtime release SHA does not match migration source SHA")
        self.checkpoint("source is not the checkout", READ_INDEXES, PARENT, source_sha="2" * 40,
                        refusal="migration source SHA does not match checkout")
        untracked = self.source / "untracked-checkpoint-note"
        untracked.write_text("not committed\n")
        self.checkpoint("checkout is dirty", READ_INDEXES, PARENT,
                        refusal="migration source checkout is dirty")
        untracked.unlink()
        self.checkpoint("current revision differs", PARENT, REVISION,
                        refusal="live Alembic current revision does not exactly match approval")
        self.checkpoint("transition skips the producer", READ_INDEXES, REVISION,
                        refusal="expected-current/target migration pair is not allowlisted")
        self.checkpoint("symbolic target", READ_INDEXES, "head",
                        refusal="checkpoint revisions must be literal IDs")
        self.sql(f"INSERT INTO alembic_version (version_num) VALUES ('{PARENT}')")
        self.checkpoint("database has two heads", READ_INDEXES, PARENT,
                        refusal="live Alembic current revision does not exactly match approval")
        self.sql(f"DELETE FROM alembic_version WHERE version_num = '{PARENT}'")
        app_as_migrator = self.workspace / "app-as-migrator.env"
        app_as_migrator.write_text(self.environment_text(migrator=STAGING_APP))
        self.checkpoint("migrator URL uses the app role", READ_INDEXES, PARENT,
                        environment_file=app_as_migrator,
                        refusal=f"migrator role must be exactly {STAGING_MIGRATOR}")

        self.checkpoint("pause at read indexes", READ_INDEXES, READ_INDEXES)
        self.checkpoint("apply producer", READ_INDEXES, PARENT)
        self.functions[PARENT] = set(self.sql(NON_EXTENSION_FUNCTIONS).splitlines())
        self.checkpoint("pause at producer", PARENT, PARENT)
        self.checkpoint("stale expected revision", READ_INDEXES, PARENT,
                        refusal="live Alembic current revision does not exactly match approval")
        self.checkpoint("apply freeze", PARENT, REVISION)
        self.functions[REVISION] = set(self.sql(NON_EXTENSION_FUNCTIONS).splitlines())
        self.checkpoint("pause at freeze", REVISION, REVISION)
        self.checkpoint("reverse transition", REVISION, PARENT,
                        refusal="expected-current/target migration pair is not allowlisted")

        self.app_select_before_convergence = self.app_can_select()
        self.convergence_refusal = self.converge("production")
        self.convergence = self.converge()
        self.role_validation = self.validate_roles()
        seed_parties(self.sql)


@pytest.fixture(scope="module")
def staging_release(tmp_path_factory):
    admin_url = os.environ.get("POSTGRES_ADMIN_TEST_DATABASE_URL")
    if not admin_url:
        pytest.skip("release checkpoint authority requires a disposable PostgreSQL cluster")
    release = SyntheticStagingRelease(make_url(admin_url), tmp_path_factory.mktemp("staging-release"))
    release.prove_isolation()
    try:
        release.create()
        release.run()
        yield release
    finally:
        release.drop()


def acl_rows(release, query, **names):
    return set(release.sql(query.format(**names)).splitlines())


def execute_as(release, role, statement, *, refused=None):
    """Run one statement on a login of `role`; with `refused`, require that SQLSTATE."""
    import asyncpg

    async def attempt():
        connection = await asyncpg.connect(release.url(role))
        try:
            assert await connection.fetchval("SELECT current_user") == role
            if refused is None:
                return await connection.execute(statement)
            with pytest.raises(asyncpg.PostgresError) as error:
                await connection.execute(statement)
            assert error.value.sqlstate == refused, (statement, error.value.sqlstate, str(error.value))
            return None
        finally:
            await connection.close()

    return asyncio.run(attempt())


def fetch_as(release, role, query):
    import asyncpg

    async def attempt():
        connection = await asyncpg.connect(release.url(role))
        try:
            return await connection.fetchval(query)
        finally:
            await connection.close()

    return asyncio.run(attempt())


def new_trade(release):
    trade_id = str(uuid4())
    insert_trade(release.sql, trade_id)
    return trade_id


def test_alembic_heads_is_one_after_both_checkpoints(staging_release):
    release = staging_release
    graph = migration_graph()
    assert graph.get_heads() == [REVISION]
    assert [item.revision for item in graph.iterate_revisions(REVISION, READ_INDEXES)] == [REVISION, PARENT]

    def children(revision):
        return [
            item.revision for item in graph.walk_revisions()
            if revision in (item.down_revision if isinstance(item.down_revision, tuple) else (item.down_revision,))
        ]

    assert children(READ_INDEXES) == [PARENT]
    assert children(PARENT) == [REVISION]
    assert children(REVISION) == []

    applied = [call for call in release.calls if call["refusal"] is None]
    assert [(call["expected"], call["target"]) for call in applied] == [
        (READ_INDEXES, READ_INDEXES), (READ_INDEXES, PARENT), (PARENT, PARENT),
        (PARENT, REVISION), (REVISION, REVISION),
    ]
    for call in applied:
        assert call["returncode"] == 0, call["output"]
        assert (f"migration checkpoint applied: {call['expected']} -> {call['target']} "
                f"at {release.source_sha}") in call["output"]
        assert call["before"] == call["expected"] and call["after"] == call["target"], call

    refused = [call for call in release.calls if call["refusal"] is not None]
    assert len(refused) == 11
    for call in refused:
        assert call["returncode"] != 0, call["label"]
        assert call["refusal"] in call["output"], (call["label"], call["output"])
        assert call["after"] == call["before"], call["label"]
        assert "migration checkpoint applied" not in call["output"], call["label"]

    assert release.revision() == REVISION
    assert release.sql("SELECT count(*) FROM alembic_version") == "1"
    assert graph.get_heads() == [REVISION]


def test_release_pin_is_an_exact_equality(tmp_path):
    from tests.unit.test_rust_release import _load_preparer

    assert json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text()) == RELEASE_ALLOWLIST

    policy_rows = {
        tuple(line.split("\t")) for line in (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
        if line and not line.startswith("#")
    }
    assert {row for row in policy_rows if PARENT in row or REVISION in row} == {
        (READ_INDEXES, PARENT), (PARENT, PARENT), (PARENT, REVISION), (REVISION, REVISION),
    }
    assert {
        ("p3_20261020_auth_tokens", "sse_20261021_replay_floor"),
        ("sse_20261021_replay_floor", "sse_20261021_replay_floor"),
        ("sse_20261021_replay_floor", READ_INDEXES), (READ_INDEXES, READ_INDEXES),
    } <= policy_rows

    preparer = _load_preparer()
    release = "a" * 40

    def project(environment, revision):
        environment_file = tmp_path / environment / ".env"
        environment_file.parent.mkdir(exist_ok=True)
        database, app_role, label, frontend = {
            "production": ("verdaxis_prod", "verdaxis_app", "prod", "https://app.verdaxis.exchange"),
            "staging": (STAGING_DATABASE, STAGING_APP, "staging", "https://staging.verdaxis.exchange"),
        }[environment]
        environment_file.write_text(
            f"ENVIRONMENT={environment}\nRELEASE_SHA={release}\n"
            f"DATABASE_URL=postgresql+asyncpg://{app_role}:fixture-secret@127.0.0.1:5432/{database}\n"
            "JWT_SECRET=" + "k" * 40 + "\n"
            f"JWT_ISSUER=verdaxis-{label}-api\nJWT_AUDIENCE=verdaxis-{label}-web\n"
            f"FRONTEND_URL={frontend}\n"
            "EMAIL_FROM=Verdaxis <noreply@verdaxis.exchange>\nRESEND_API_KEY=re_fixture_key\n"
        )
        policy = {"environment": environment, "db_session_time_zone": "UTC"}
        return preparer.project_runtime_environment(environment_file, release, revision, policy)

    for revision in RELEASE_ALLOWLIST:
        if revision in PRODUCTION_CHECKPOINTS:
            assert project("production", revision)["MIGRATION_REVISION"] == revision
        else:
            with pytest.raises(preparer.RustRuntimeError, match="reviewed union checkpoint"):
                project("production", revision)
    for revision in NON_LITERAL_REVISIONS:
        with pytest.raises(preparer.RustRuntimeError, match="reviewed union checkpoint"):
            project("production", revision)
    # The staging preparer copies the revision; the compiled Rust allowlist and the
    # checkpoint helper are the staging admission boundaries.
    for revision in (*RELEASE_ALLOWLIST, "unlisted_revision"):
        assert project("staging", revision)["MIGRATION_REVISION"] == revision


def test_release_artifact_revision_binds_the_live_database(staging_release):
    from dotenv import dotenv_values

    release = staging_release
    unit = (ROOT / "deploy/systemd/verdaxis-backend-staging.service").read_text()
    assert "EnvironmentFile=/home/verdaxis-prod/verdaxis/staging/be/.runtime-release.env" in unit
    assert "scripts/verify_migration_revision.py --expected ${MIGRATION_REVISION}" in unit

    def start_with_artifact(revision):
        artifact = release.workspace / ".runtime-release.env"
        artifact.write_text(f"ENVIRONMENT=staging\nRELEASE_SHA={release.source_sha}\nMIGRATION_REVISION={revision}\n")
        # The unit loads .env, then the release artifact, into the process environment.
        environment = {**dotenv_values(release.environment_file), **dotenv_values(artifact)}
        result = subprocess.run(
            [sys.executable, "scripts/verify_migration_revision.py",
             "--expected", environment["MIGRATION_REVISION"]],
            cwd=release.source,
            env={"PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1", **environment},
            capture_output=True, text=True, check=False, timeout=300,
        )
        return result.returncode, result.stdout + result.stderr

    returncode, output = start_with_artifact(REVISION)
    assert returncode == 0, output
    assert f"runtime migration checkpoint verified: {REVISION}" in output
    for revision, refusal in (
        (PARENT, "does not match deployed checkpoint"),
        (READ_INDEXES, "does not match deployed checkpoint"),
        ("head", "literal IDs"),
        ("unlisted_revision", "does not match deployed checkpoint"),
    ):
        returncode, output = start_with_artifact(revision)
        assert returncode != 0 and refusal in output, (revision, output)
    assert release.revision() == REVISION


def test_trade_freezes_acl_matrix(staging_release):
    release = staging_release
    assert release.app_select_before_convergence == {"trade_freezes": "f", "watchlist_producer_state": "f"}
    assert release.convergence_refusal.returncode != 0
    assert "does not match the selected application database identity" in release.convergence_refusal.stderr
    assert release.convergence.returncode == 0, release.convergence.stdout + release.convergence.stderr
    assert (f"runtime ACL convergence complete: {STAGING_DATABASE} app={STAGING_APP} "
            f"backup={STAGING_BACKUP}") in release.convergence.stdout
    assert release.role_validation.returncode == 0, release.role_validation.stdout + release.role_validation.stderr

    assert acl_rows(release, NON_OWNER_TABLE_ACL, table="trade_freezes") == {
        f"{STAGING_APP}:INSERT", f"{STAGING_APP}:SELECT", f"{STAGING_BACKUP}:SELECT",
    }
    assert acl_rows(release, COLUMN_ACL, table="trade_freezes") == {
        f"{STAGING_APP}:UPDATE:{column}" for column in UPDATES
    }
    owner = release.sql("SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid = 'trade_freezes'::regclass")
    assert owner == STAGING_MIGRATOR
    for role, privileges in (
        (STAGING_APP, {"SELECT", "INSERT"}), (STAGING_BACKUP, {"SELECT"}), ("public", set()),
    ):
        for privilege in ("SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE", "REFERENCES", "TRIGGER"):
            expected = "t" if privilege in privileges else "f"
            assert release.sql(
                f"SELECT has_table_privilege('{role}','trade_freezes','{privilege}')"
            ) == expected, (role, privilege)

    trade_id = new_trade(release)
    freeze_id = str(uuid4())
    execute_as(release, STAGING_APP, freeze_insert(freeze_id, trade_id))
    assert fetch_as(release, STAGING_APP, f"SELECT status FROM trade_freezes WHERE id='{freeze_id}'") == "ACTIVE"
    for column, value in {
        "id": "gen_random_uuid()", "trade_id": f"'{trade_id}'", "reason_code": "'COURT_ORDER'",
        "note": "'Changed history'", "frozen_by_user_id": f"'{ACTOR}'", "frozen_at": "now()",
    }.items():
        execute_as(release, STAGING_APP, f"UPDATE trade_freezes SET {column}={value} WHERE id='{freeze_id}'",
                   refused="42501")
    for statement in ("DELETE FROM trade_freezes WHERE false", "TRUNCATE trade_freezes"):
        execute_as(release, STAGING_APP, statement, refused="42501")
        execute_as(release, STAGING_BACKUP, statement, refused="42501")
    execute_as(release, STAGING_BACKUP, freeze_insert(None, trade_id), refused="42501")
    execute_as(release, STAGING_BACKUP, "UPDATE trade_freezes SET version=version WHERE false", refused="42501")
    assert fetch_as(release, STAGING_BACKUP, f"SELECT count(*) FROM trade_freezes WHERE id='{freeze_id}'") == 1
    execute_as(release, STAGING_APP, f"""UPDATE trade_freezes SET status='CLEARED', cleared_at=now(),
        cleared_by_user_id='{ACTOR}', clear_note='Review complete', version=version+1
        WHERE id='{freeze_id}'""")
    assert fetch_as(release, STAGING_APP, f"SELECT status FROM trade_freezes WHERE id='{freeze_id}'") == "CLEARED"

    # Table-level SELECT and INSERT cover a column added later; UPDATE stays column-scoped.
    release.sql("ALTER TABLE trade_freezes ADD COLUMN review_reference text")
    try:
        for privilege, expected in (("SELECT", "t"), ("INSERT", "t"), ("UPDATE", "f")):
            assert release.sql(
                f"SELECT has_column_privilege('{STAGING_APP}','trade_freezes','review_reference','{privilege}')"
            ) == expected, privilege
        reconverged = release.converge()
        assert reconverged.returncode == 0, reconverged.stdout + reconverged.stderr
        assert acl_rows(release, COLUMN_ACL, table="trade_freezes") == {
            f"{STAGING_APP}:UPDATE:{column}" for column in UPDATES
        }
        execute_as(release, STAGING_APP, "UPDATE trade_freezes SET review_reference='x' WHERE false",
                   refused="42501")
    finally:
        release.sql("ALTER TABLE trade_freezes DROP COLUMN IF EXISTS review_reference")


def test_convergence_refuses_wrong_owner_and_missing_declared_column(staging_release):
    release = staging_release
    before = acl_rows(release, NON_OWNER_TABLE_ACL, table="trade_freezes") | acl_rows(
        release, COLUMN_ACL, table="trade_freezes")
    release.sql("CREATE TABLE public.convergence_owner_probe (id integer)")
    try:
        refused = release.converge()
        assert refused.returncode != 0
        assert "owner-executable runtime ACL convergence failed" in refused.stderr
    finally:
        release.sql("DROP TABLE public.convergence_owner_probe")
    release.sql("ALTER TABLE trade_freezes RENAME COLUMN clear_note TO clear_note_renamed")
    try:
        refused = release.converge()
        assert refused.returncode != 0
        assert "owner-executable runtime ACL convergence failed" in refused.stderr
    finally:
        release.sql("ALTER TABLE trade_freezes RENAME COLUMN clear_note_renamed TO clear_note")
    after = acl_rows(release, NON_OWNER_TABLE_ACL, table="trade_freezes") | acl_rows(
        release, COLUMN_ACL, table="trade_freezes")
    assert after == before


def test_producer_state_and_outbox_acl_matrix(staging_release):
    release = staging_release
    assert release.convergence.returncode == 0, release.convergence.stderr
    for table in ("watchlist_producer_state", "market_event_outbox"):
        assert acl_rows(release, NON_OWNER_TABLE_ACL, table=table) == {
            f"{STAGING_APP}:INSERT", f"{STAGING_APP}:SELECT", f"{STAGING_APP}:UPDATE",
            f"{STAGING_BACKUP}:SELECT",
        }, table
        assert acl_rows(release, COLUMN_ACL, table=table) == set(), table
        for role, privileges in (
            (STAGING_APP, {"SELECT", "INSERT", "UPDATE"}), (STAGING_BACKUP, {"SELECT"}), ("public", set()),
        ):
            for privilege in ("SELECT", "INSERT", "UPDATE", "DELETE", "TRUNCATE"):
                expected = "t" if privilege in privileges else "f"
                assert release.sql(
                    f"SELECT has_table_privilege('{role}','{table}','{privilege}')"
                ) == expected, (table, role, privilege)
    for privilege in ("SELECT", "INSERT", "UPDATE"):
        assert release.sql(
            f"SELECT has_column_privilege('{STAGING_APP}','market_event_outbox','producer_evidence','{privilege}')"
        ) == "t", privilege

    execute_as(release, STAGING_APP,
               "INSERT INTO watchlist_producer_state (singleton) VALUES (true) ON CONFLICT DO NOTHING")
    execute_as(release, STAGING_APP, "UPDATE watchlist_producer_state SET consumed_through_seq = consumed_through_seq + 1")
    assert fetch_as(release, STAGING_APP, "SELECT count(*) FROM watchlist_producer_state") == 1
    execute_as(release, STAGING_APP, "UPDATE watchlist_producer_state SET consumed_through_seq = -1",
               refused="23514")
    for statement in ("DELETE FROM watchlist_producer_state WHERE false", "TRUNCATE watchlist_producer_state"):
        execute_as(release, STAGING_APP, statement, refused="42501")
    execute_as(release, STAGING_BACKUP, "UPDATE watchlist_producer_state SET singleton = true WHERE false",
               refused="42501")

    source = str(uuid4())
    execute_as(release, STAGING_APP, f"""INSERT INTO market_event_outbox
        (id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload)
        VALUES ('{source}','trade.updated','trade','acceptance','["{BUYER}"]','{{}}')""")
    assert fetch_as(release, STAGING_APP,
                    f"SELECT producer_evidence IS NULL FROM market_event_outbox WHERE id='{source}'") is True
    # The shared runtime role keeps the Phase 6 writer and sequencer UPDATE path.
    execute_as(release, STAGING_APP, "UPDATE market_event_outbox SET producer_evidence = producer_evidence WHERE false")
    for statement in ("DELETE FROM market_event_outbox WHERE false", "TRUNCATE market_event_outbox"):
        execute_as(release, STAGING_APP, statement, refused="42501")
        execute_as(release, STAGING_BACKUP, statement, refused="42501")
    assert fetch_as(release, STAGING_BACKUP, f"SELECT count(*) FROM market_event_outbox WHERE id='{source}'") == 1


def test_freeze_trigger_is_enabled(staging_release):
    release = staging_release
    triggers = dict(
        line.split("|", 1) for line in release.sql("""
            SELECT tgname || '|' || tgenabled::text || '|' || pg_get_triggerdef(oid)
            FROM pg_trigger WHERE tgname IN ('trades_freeze_guard', 'trade_freezes_history_guard')
            ORDER BY tgname""").splitlines()
    )
    assert triggers == {
        "trade_freezes_history_guard": "O|CREATE TRIGGER trade_freezes_history_guard BEFORE UPDATE "
        "ON public.trade_freezes FOR EACH ROW EXECUTE FUNCTION trade_freezes_history_guard()",
        "trades_freeze_guard": "O|CREATE TRIGGER trades_freeze_guard BEFORE UPDATE OF status "
        "ON public.trades FOR EACH ROW EXECUTE FUNCTION trades_freeze_guard()",
    }
    for function in FREEZE_FUNCTIONS:
        assert release.sql(f"""SELECT pg_get_userbyid(proowner) || '|' || prosecdef::text || '|' ||
            array_to_string(proconfig, ';') FROM pg_proc WHERE oid = 'public.{function}()'::regprocedure""") == (
            f"{STAGING_MIGRATOR}|false|search_path=pg_catalog, public"
        )

    # Neither runtime role can turn the ordinary triggers off.
    for role in (STAGING_APP, STAGING_BACKUP):
        execute_as(release, role, "SET session_replication_role = replica", refused="42501")
    execute_as(release, STAGING_APP, "ALTER TABLE trades DISABLE TRIGGER trades_freeze_guard", refused="42501")
    execute_as(release, STAGING_APP, "ALTER TABLE trade_freezes DISABLE TRIGGER ALL", refused="42501")

    trade_id = new_trade(release)
    freeze_id = str(uuid4())
    execute_as(release, STAGING_APP, freeze_insert(freeze_id, trade_id))
    for status in ("CONFIRMED", "DELIVERED", "PAID", "DECLINED", "CANCELLED"):
        execute_as(release, STAGING_APP, f"UPDATE trades SET status='{status}' WHERE id='{trade_id}'",
                   refused="ZF001")
    execute_as(release, STAGING_APP, f"UPDATE trades SET status=status WHERE id='{trade_id}'")
    assert release.sql(f"SELECT status FROM trades WHERE id='{trade_id}'") == "PENDING_CONFIRMATION"

    # A clearance must advance the version; a cleared row then refuses every material change.
    execute_as(release, STAGING_APP, f"""UPDATE trade_freezes SET status='CLEARED', cleared_at=now(),
        cleared_by_user_id='{ACTOR}', clear_note='Review complete' WHERE id='{freeze_id}'""", refused="ZF002")
    execute_as(release, STAGING_APP, f"UPDATE trade_freezes SET status=status WHERE id='{freeze_id}'")
    execute_as(release, STAGING_APP, f"""UPDATE trade_freezes SET status='CLEARED', cleared_at=now(),
        cleared_by_user_id='{ACTOR}', clear_note='Review complete', version=version+1 WHERE id='{freeze_id}'""")
    execute_as(release, STAGING_APP, f"UPDATE trade_freezes SET status=status WHERE id='{freeze_id}'")
    for assignment in ("status='ACTIVE', cleared_at=NULL, cleared_by_user_id=NULL, clear_note=NULL",
                       "clear_note='Changed clearance'", "version=version+1"):
        execute_as(release, STAGING_APP, f"UPDATE trade_freezes SET {assignment} WHERE id='{freeze_id}'",
                   refused="ZF003")
    # History fields are checked before the cleared-row rule.
    with pytest.raises(AssertionError, match="freeze history is immutable"):
        release.sql(f"UPDATE trade_freezes SET note='Changed history' WHERE id='{freeze_id}'")
    execute_as(release, STAGING_APP,
               f"UPDATE trades SET status='CONFIRMED', confirmed_at=now() WHERE id='{trade_id}'")
    assert release.sql(f"SELECT status FROM trades WHERE id='{trade_id}'") == "CONFIRMED"

    second = new_trade(release)
    execute_as(release, STAGING_APP, freeze_insert(None, second))
    execute_as(release, STAGING_APP, freeze_insert(None, second), refused="23505")
    assert release.sql(f"SELECT count(*) FROM trade_freezes WHERE trade_id='{second}' AND status='ACTIVE'") == "1"


def test_app_role_cannot_delete_or_execute_functions(staging_release):
    release = staging_release
    assert release.functions[PARENT] == release.functions[READ_INDEXES]
    assert release.functions[REVISION] - release.functions[PARENT] == FREEZE_FUNCTIONS
    assert release.functions[PARENT] <= release.functions[REVISION]
    for function in FREEZE_FUNCTIONS:
        assert acl_rows(release, FUNCTION_ACL, function=function) == {f"{STAGING_MIGRATOR}:EXECUTE"}
        for role in (STAGING_APP, STAGING_BACKUP, "public"):
            assert release.sql(f"SELECT has_function_privilege('{role}','public.{function}()','EXECUTE')") == "f"
        for role in (STAGING_APP, STAGING_BACKUP):
            execute_as(release, role, f"SELECT public.{function}()", refused="42501")
    assert release.sql(f"""SELECT count(*) FROM pg_auth_members AS membership
        JOIN pg_roles AS granted ON granted.oid = membership.roleid
        JOIN pg_roles AS member ON member.oid = membership.member
        WHERE granted.rolname IN ('{STAGING_APP}','{STAGING_MIGRATOR}','{STAGING_BACKUP}')
           OR member.rolname IN ('{STAGING_APP}','{STAGING_MIGRATOR}','{STAGING_BACKUP}')""") == "0"
    assert release.sql(f"""SELECT bool_and(NOT rolinherit AND NOT rolsuper) FROM pg_roles
        WHERE rolname IN ('{STAGING_APP}','{STAGING_MIGRATOR}','{STAGING_BACKUP}')""") == "t"
    for table in ("trade_freezes", "watchlist_producer_state", "market_event_outbox"):
        for role in (STAGING_APP, STAGING_BACKUP):
            execute_as(release, role, f"DELETE FROM {table} WHERE false", refused="42501")
            execute_as(release, role, f"ALTER TABLE {table} ADD COLUMN acceptance_probe text", refused="42501")
            execute_as(release, role, f"DROP TABLE {table}", refused="42501")
        assert fetch_as(release, STAGING_BACKUP, f"SELECT count(*) >= 0 FROM {table}") is True


def test_producer_downgrade_waits_for_its_locks_in_source_order(supplier_scratch_database):
    import asyncpg

    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    url = database_url(sql)

    async def observe():
        holder = await asyncpg.connect(url)
        observer = await asyncpg.connect(url)
        pool = ThreadPoolExecutor(max_workers=1)
        future = None
        try:
            transaction = holder.transaction()
            await transaction.start()
            await holder.execute("LOCK TABLE watchlist_events IN ACCESS SHARE MODE")
            future = pool.submit(migrate, "downgrade", READ_INDEXES)
            for _ in range(1000):
                rows = await observer.fetch("""
                    SELECT relation::regclass::text AS relation, granted FROM pg_locks
                    WHERE mode = 'AccessExclusiveLock' AND pid <> pg_backend_pid()
                      AND relation IN ('market_event_outbox'::regclass, 'watchlist_events'::regclass)""")
                state = {(row["relation"], row["granted"]) for row in rows}
                if ("watchlist_events", False) in state:
                    break
                await asyncio.sleep(0.01)
            else:
                pytest.fail("producer downgrade did not wait for the derived event table")
            # The source outbox lock is held while the derived table lock waits.
            assert ("market_event_outbox", True) in state, state
            assert not future.done()
            await transaction.rollback()
            await asyncio.wrap_future(future)
        finally:
            await holder.close()
            await observer.close()
            pool.shutdown(wait=True)

    asyncio.run(observe())
    assert sql("SELECT version_num FROM alembic_version") == READ_INDEXES
    assert sql("SELECT to_regclass('watchlist_producer_state') IS NULL") == "t"


def test_freeze_downgrade_fails_fast_when_its_tables_are_busy(freeze_database):
    import asyncpg

    sql, migrate = freeze_database
    before = catalog(sql)
    url = database_url(sql)

    async def attempt():
        holder = await asyncpg.connect(url)
        try:
            transaction = holder.transaction()
            await transaction.start()
            await holder.execute("LOCK TABLE trade_freezes IN ACCESS SHARE MODE")
            output = await asyncio.to_thread(migrate, "downgrade", PARENT, success=False)
            await transaction.rollback()
            return output
        finally:
            await holder.close()

    output = asyncio.run(attempt())
    assert "could not obtain lock on relation" in output, output
    assert "lock timeout" not in output, output
    assert sql("SELECT version_num FROM alembic_version") == REVISION
    assert catalog(sql) == before
