"""Trade freeze schema, retained history, runtime grants and release checkpoints."""

import ast
import asyncio
import json
import os
import re
import subprocess
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
    assert migration_graph().get_revision(PARENT).down_revision == "p3_20261020_auth_tokens"


@pytest.fixture
def freeze_database(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    assert MIGRATION.is_file(), "trade freeze revision is absent"
    migrate("upgrade", REVISION)
    return sql, migrate


def seed_trade(sql):
    sql(f"""
        INSERT INTO users (id,email,password_hash,role)
        VALUES ('{ACTOR}','freeze-admin@example.test','unused','ADMIN');
        INSERT INTO organizations (id,name,type,provenance) VALUES
          ('{BUYER}','Freeze buyer','FUEL_BUYER','REAL'),
          ('{SELLER}','Freeze seller','FUEL_SUPPLIER','REAL');
        INSERT INTO trades (id,buyer_id,seller_id,initiator_org_id,initiated_by,
          buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,
          market_product,delivery_point_id,delivery_point_name,delivery_point_region,
          availability_window,quantity_mt,price_per_mt_usd,status,commission_rate_pct)
        SELECT '{TRADE}','{BUYER}','{SELLER}','{BUYER}','BUYER','REAL','REAL',
          p.id,p.name,p.fuel_type,p.fuel_grade,'BIO_METHANOL',d.id,d.name,d.region,
          'SPOT',10,700,'PENDING_CONFIRMATION',0
        FROM products p CROSS JOIN delivery_points d
        WHERE p.code='BIO_METHANOL' AND d.name='Singapore';
    """)
    assert sql("SELECT count(*) FROM trades") == "1"


def freeze_insert(freeze_id=None):
    return f"""INSERT INTO trade_freezes
        (id,trade_id,status,reason_code,note,frozen_by_user_id)
        VALUES ('{freeze_id or uuid4()}','{TRADE}','ACTIVE','OTHER','Review required','{ACTOR}')"""


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
        "relations": sql("""SELECT relname || ':' || relkind FROM pg_class
            WHERE relnamespace='public'::regnamespace ORDER BY 1""").splitlines(),
        "triggers": sql("""SELECT tgrelid::regclass || ':' || tgname || ':' ||
            tgenabled || ':' || pg_get_triggerdef(oid) FROM pg_trigger
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
    parents = {item.down_revision for item in graph.walk_revisions() if item.revision != REVISION}
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


def test_release_pin_lists_the_new_head():
    revisions = json.loads((ROOT / "deploy/rust/allowed-revisions.json").read_text())
    assert revisions == ["kycg_20261014_staging", "p0_20261016_union", "prc_20261018_cache", PARENT, REVISION]
    checkpoints = {tuple(line.split()) for line in (ROOT / "deploy/migration-checkpoints.tsv").read_text().splitlines()
                   if line and not line.startswith("#")}
    assert (PARENT, REVISION) in checkpoints
    assert (REVISION, REVISION) in checkpoints
    for path in ("rs/read/src/runtime/config.rs", "scripts/prepare_rust_runtime.py"):
        source = (ROOT / path).read_text()
        assert f'migration_revision != "{REVISION}"' in source
        assert f'migration_revision != "{PARENT}"' not in source
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
                        VALUES ('{uuid4()}','trade.updated','trade','{TRADE}','[]','{{}}')""")
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
        ("'ACTIVE'", "'PAUSED'", "ck_trade_freezes_status"),
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
