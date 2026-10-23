"""Producer identity, replay keys, runtime authority and reversible schema additions."""
import json
import os
import subprocess
from pathlib import Path

import pytest
from sqlalchemy.engine import make_url

from tests.postgres.test_supplier_offer_migration_roundtrip import (
    supplier_scratch_database,  # noqa: F401
)

ROOT = Path(__file__).resolve().parents[2]
PARENT = "prc_20261018_cache"
REVISION = "p89_20261022_watchlist_producer"
WATCHLIST = "88888888-7777-0000-0000-000000000002"
TARGET = "88888888-7777-0000-0000-000000000003"
EVENT = "88888888-7777-0000-0000-000000000004"
SOURCE = "88888888-7777-0000-0000-000000000005"
IDENTITY_COLUMNS = {"producer", "source_event_id", "source_stream_seq", "scope_attested_at", "derived_event_key"}


def seed_target(sql):
    sql(f"""
        INSERT INTO users (id,email,password_hash,role)
        VALUES ('88888888-7777-0000-0000-000000000001','observer@example.test','unused','BUYER');
        INSERT INTO watchlists (id,user_id,name,kind)
        VALUES ('{WATCHLIST}','88888888-7777-0000-0000-000000000001','Schema fixture','CUSTOM');
        INSERT INTO watchlist_targets (id,watchlist_id,target_type,market_product_code,
            delivery_point_id,availability_window_code)
        VALUES ('{TARGET}','{WATCHLIST}','SLICE','BIO_METHANOL',
            '73835e92-820e-584b-8280-bb61c63aa28e','SPOT');
    """)


def insert_event(*, identity="", values="", event=EVENT):
    return f"""INSERT INTO watchlist_events
        (id,watchlist_id,watchlist_target_id,event_type{identity})
        VALUES ('{event}','{WATCHLIST}','{TARGET}','PIN_PRICE_CHANGED'{values})"""


def upgrade_if_present(migrate):
    # RED uses the real parent schema; absence is reported by catalog assertions.
    if (ROOT / f"alembic/versions/{REVISION}.py").is_file():
        migrate("upgrade", REVISION)


def test_producer_identity_check_refuses_a_half_attested_row(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    upgrade_if_present(migrate)
    definition = sql("""SELECT COALESCE((SELECT pg_get_constraintdef(oid)
        FROM pg_constraint WHERE conrelid='watchlist_events'::regclass
        AND conname='ck_watchlist_events_producer_identity'), '')""")
    assert definition, "ck_watchlist_events_producer_identity is absent"
    assert all(column in definition for column in IDENTITY_COLUMNS), definition
    seed_target(sql)
    for identity, values in (
        (",producer", ",'rust-market-v1'"),
        (",source_event_id", f",'{SOURCE}'"),
        (",producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key",
         f",'rust-market-v1','{SOURCE}',1,now(),''"),
    ):
        with pytest.raises(AssertionError, match="ck_watchlist_events_producer_identity"):
            sql(insert_event(identity=identity, values=values))
    with pytest.raises(AssertionError, match="check constraint"):
        sql(insert_event(identity=",producer", values=",'other'"))


def test_legacy_rows_backfill_to_legacy_python(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    seed_target(sql)
    sql(insert_event())
    upgrade_if_present(migrate)
    columns = sql("SELECT column_name FROM information_schema.columns WHERE table_name='watchlist_events'")
    assert IDENTITY_COLUMNS <= set(columns.splitlines())
    assert sql("""SELECT producer || ':' || (source_event_id IS NULL AND source_stream_seq IS NULL
        AND scope_attested_at IS NULL AND derived_event_key IS NULL)::text FROM watchlist_events""") == "legacy-python:true"
    sql(insert_event(event="88888888-7777-0000-0000-000000000006"))
    assert sql("SELECT count(*) FROM watchlist_events WHERE producer='legacy-python'") == "2"


def test_producer_schema_revision_is_additive_and_single_headed(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    before = sql("""SELECT json_agg(row_to_json(c) ORDER BY table_name,column_name) FROM
        (SELECT table_name,column_name,data_type,is_nullable,column_default FROM information_schema.columns
         WHERE table_schema='public') c""")
    triggers = sql("SELECT oid || ':' || pg_get_triggerdef(oid) FROM pg_trigger WHERE NOT tgisinternal ORDER BY oid")
    upgrade_if_present(migrate)
    assert sql("SELECT version_num FROM alembic_version") == REVISION
    from alembic.config import Config
    from alembic.script import ScriptDirectory
    graph = ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))
    assert graph.get_heads() == [REVISION]
    assert graph.get_revision(REVISION).down_revision == PARENT
    after = json.loads(sql("""SELECT json_agg(row_to_json(c) ORDER BY table_name,column_name) FROM
        (SELECT table_name,column_name,data_type,is_nullable,column_default FROM information_schema.columns
         WHERE table_schema='public') c"""))
    assert all(column in after for column in json.loads(before))
    assert sql("SELECT oid || ':' || pg_get_triggerdef(oid) FROM pg_trigger WHERE NOT tgisinternal ORDER BY oid") == triggers
    seed_target(sql)
    trusted = insert_event(
        identity=",producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key",
        values=f",'rust-market-v1','{SOURCE}',1,now(),'PIN_PRICE_CHANGED|order|ASK|0'",
    )
    sql(trusted)
    with pytest.raises(AssertionError, match="uq_watchlist_events_producer_source_target_derived"):
        sql(trusted.replace(EVENT, "88888888-7777-0000-0000-000000000007"))
    sql(trusted.replace(EVENT, "88888888-7777-0000-0000-000000000008").replace("PIN_PRICE_CHANGED|", "PIN_QUANTITY_CHANGED|"))
    assert sql("SELECT count(*) FROM watchlist_events") == "2"


def test_producer_downgrade_refuses_while_trusted_rows_exist(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    baseline = sql("SELECT table_name || '.' || column_name FROM information_schema.columns WHERE table_schema='public' ORDER BY 1")
    upgrade_if_present(migrate)
    assert sql("SELECT to_regclass('watchlist_producer_state') IS NOT NULL") == "t"
    seed_target(sql)
    sql(insert_event(identity=",producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key",
                     values=f",'rust-market-v1','{SOURCE}',1,now(),'PIN_PRICE_CHANGED|order|ASK|0'"))
    refusal = migrate("downgrade", PARENT, success=False)
    assert "Retained producer evidence prevents downgrade" in refusal
    assert sql("SELECT version_num FROM alembic_version") == REVISION
    sql("DELETE FROM watchlist_events")
    sql(f"""INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids,producer_evidence)
        VALUES ('{SOURCE}','order.updated','order','schema-fixture','{{}}','[\"88888888-7777-0000-0000-000000000009\"]','{{\"version\":1}}')""")
    refusal = migrate("downgrade", PARENT, success=False)
    assert "Retained producer evidence prevents downgrade" in refusal
    assert sql("SELECT producer_evidence->>'version' FROM market_event_outbox") == "1"
    sql("DELETE FROM market_event_outbox")
    sql("INSERT INTO watchlist_producer_state (singleton) VALUES (true)")
    migrate("downgrade", PARENT)
    assert sql("SELECT version_num FROM alembic_version") == PARENT
    assert sql("SELECT table_name || '.' || column_name FROM information_schema.columns WHERE table_schema='public' ORDER BY 1") == baseline
    assert sql("SELECT to_regclass('uq_watchlist_events_producer_source_target_derived') IS NULL") == "t"
    migrate("upgrade", REVISION)


def test_internal_evidence_column_is_additive_and_python_writes_default_null(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    upgrade_if_present(migrate)
    assert sql("""SELECT data_type || ':' || is_nullable FROM information_schema.columns
        WHERE table_name='market_event_outbox' AND column_name='producer_evidence'""") == "jsonb:YES"
    sql(f"""INSERT INTO market_event_outbox (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids)
        VALUES ('{SOURCE}','order.updated','order','schema-fixture','{{\"schema_version\":1}}',
        '[\"88888888-7777-0000-0000-000000000009\"]')""")
    assert sql("SELECT producer_evidence IS NULL FROM market_event_outbox") == "t"
    assert json.loads(sql("SELECT payload FROM market_event_outbox")) == {"schema_version": 1}


def test_python_entity_flush_keeps_original_insert_and_returning_columns(supplier_scratch_database):
    from uuid import UUID

    from sqlalchemy import event
    from sqlalchemy.ext.asyncio import AsyncSession, create_async_engine

    from app.models.market_event import MarketEventOutbox
    from app.models.watchlist import WatchlistEvent, WatchlistEventType

    sql, migrate = supplier_scratch_database
    migrate("upgrade", REVISION)
    seed_target(sql)
    url = make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"]).set(
        database=sql("SELECT current_database()")
    ).render_as_string(hide_password=False)
    statements = []

    async def flush():
        engine = create_async_engine(url)
        @event.listens_for(engine.sync_engine, "before_cursor_execute")
        def capture(connection, cursor, statement, parameters, context, executemany):
            if statement.startswith("INSERT INTO"):
                statements.append(statement)
        try:
            async with AsyncSession(engine) as session:
                session.add(WatchlistEvent(
                    watchlist_id=UUID(WATCHLIST), watchlist_target_id=UUID(TARGET),
                    event_type=WatchlistEventType.PIN_PRICE_CHANGED,
                ))
                session.add(MarketEventOutbox(
                    event_type="order.updated", aggregate_type="order", aggregate_id="fixture",
                    participant_org_ids=["88888888-7777-0000-0000-000000000009"], payload={},
                ))
                await session.flush()
                await session.rollback()
        finally:
            await engine.dispose()
    asyncio.run(flush())
    expected = {
        "watchlist_events": ["id", "watchlist_id", "watchlist_target_id", "event_type", "event_payload", "is_read", "created_at"],
        "market_event_outbox": ["id", "event_type", "aggregate_type", "aggregate_id", "participant_org_ids", "payload", "created_at", "dispatched_at", "stream_seq", "delivery_attempts", "last_error"],
    }
    assert len(statements) == 2, statements
    for statement in statements:
        table = statement.split()[2]
        assert statement.split("(", 1)[1].split(")", 1)[0].split(", ") == expected[table]
        assert "RETURNING" not in statement, statement


def test_producer_metadata_matches_migrated_head(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", REVISION)
    url = make_url(os.environ["MARKET_INTEGRITY_TEST_DATABASE_URL"]).set(
        database=sql("SELECT current_database()")
    ).render_as_string(hide_password=False)
    result = subprocess.run(
        [sys.executable, "-m", "alembic", "check"], cwd=ROOT,
        env={**os.environ, "DATABASE_URL": url, "MIGRATOR_DATABASE_URL": url},
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr


@pytest.mark.parametrize("relation", ["market_event_outbox", "watchlist_events"])
def test_concurrent_trusted_writer_is_not_removed_by_downgrade(supplier_scratch_database, relation):
    import asyncpg

    sql, migrate = supplier_scratch_database
    migrate("upgrade", REVISION)
    seed_target(sql)
    database = sql("SELECT current_database()")
    admin = make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    url = admin.set(drivername="postgresql", database=database).render_as_string(hide_password=False)

    async def race():
        writer = await asyncpg.connect(url)
        observer = await asyncpg.connect(url)
        transaction = writer.transaction()
        await transaction.start()
        pool = ThreadPoolExecutor(max_workers=1)
        future = None
        try:
            # A table reader blocks the first DDL but permits a trusted INSERT.
            await writer.execute(f"LOCK TABLE {relation} IN ACCESS SHARE MODE")
            future = pool.submit(migrate, "downgrade", PARENT, success=False)
            deadline = asyncio.get_running_loop().time() + 10
            while asyncio.get_running_loop().time() < deadline:
                waiting = await observer.fetchval("""SELECT EXISTS (
                    SELECT 1 FROM pg_locks WHERE relation=$1::regclass
                    AND mode='AccessExclusiveLock' AND NOT granted)""", relation)
                if waiting:
                    break
                await asyncio.sleep(0.01)
            else:
                pytest.fail("downgrade did not reach the table-lock barrier")
            if relation == "market_event_outbox":
                await writer.execute(f"""INSERT INTO market_event_outbox
                    (id,event_type,aggregate_type,aggregate_id,payload,participant_org_ids,producer_evidence)
                    VALUES ('{SOURCE}','order.updated','order','schema-fixture','{{}}',
                    '[\"88888888-7777-0000-0000-000000000009\"]','{{\"version\":1}}')""")
            else:
                await writer.execute(insert_event(
                    identity=",producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key",
                    values=f",'rust-market-v1','{SOURCE}',1,now(),'PIN_PRICE_CHANGED|order|ASK|0'",
                ))
            await transaction.commit()
            output = await asyncio.wrap_future(future)
            assert "Retained producer evidence prevents downgrade" in output
            assert sql("SELECT version_num FROM alembic_version") == REVISION
            if relation == "market_event_outbox":
                assert sql("SELECT producer_evidence->>'version' FROM market_event_outbox") == "1"
            else:
                assert sql("SELECT producer FROM watchlist_events") == "rust-market-v1"
        finally:
            if not writer.is_closed():
                await writer.close()
            await observer.close()
            pool.shutdown(wait=True)

    asyncio.run(race())


def test_watermark_acl_denies_delete(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", PARENT)
    upgrade_if_present(migrate)
    assert sql("SELECT to_regclass('watchlist_producer_state') IS NOT NULL") == "t"
    app = os.environ["RUNTIME_TEST_APP_ROLE"]
    migrator = os.environ["RUNTIME_TEST_MIGRATOR_ROLE"]
    backup = os.environ["RUNTIME_TEST_BACKUP_ROLE"]
    database = sql("SELECT current_database()")
    # Use the real convergence policy, not grants duplicated in the test.
    sql(f"ALTER SCHEMA public OWNER TO {migrator}")
    admin = make_url(os.environ["POSTGRES_ADMIN_TEST_DATABASE_URL"])
    result = subprocess.run(
        ["psql", "-X", "-h", admin.host, "-p", str(admin.port), "-U", admin.username,
         "-d", database, "--single-transaction", "-v", "ON_ERROR_STOP=1",
         "-v", f"database_name={database}", "-v", f"app_role={app}",
         "-v", f"migrator_role={migrator}", "-v", f"backup_role={backup}",
         "-f", str(ROOT / "deploy/postgres/converge_runtime_object_acls.sql")],
        env={**os.environ, "PGPASSWORD": admin.password},
        capture_output=True, text=True, check=False,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    assert sql(f"SELECT has_table_privilege('{app}','watchlist_producer_state','SELECT,INSERT,UPDATE')") == "t"
    sql(f"SET ROLE {app}; INSERT INTO watchlist_producer_state (singleton) VALUES (true); UPDATE watchlist_producer_state SET consumed_through_seq=1; SELECT consumed_through_seq FROM watchlist_producer_state")
    with pytest.raises(AssertionError, match="permission denied"):
        sql(f"SET ROLE {app}; DELETE FROM watchlist_producer_state")
    with pytest.raises(AssertionError, match="permission denied"):
        sql(f"SET ROLE {app}; DELETE FROM market_event_outbox")
    assert sql(f"SELECT has_table_privilege('{app}','market_event_outbox','SELECT,INSERT,UPDATE')") == "t"
    assert sql(f"SELECT has_table_privilege('{app}','watchlist_targets','SELECT,UPDATE')") == "t"
    assert sql(f"SELECT has_column_privilege('{app}','market_event_outbox','producer_evidence','INSERT')") == "t"
    sql(f"SET ROLE {app}; SELECT producer_evidence FROM market_event_outbox")
    with pytest.raises(AssertionError, match="check constraint"):
        sql(f"SET ROLE {app}; UPDATE watchlist_producer_state SET consumed_through_seq=-1")
    with pytest.raises(AssertionError, match="check constraint"):
        sql(f"SET ROLE {app}; INSERT INTO watchlist_producer_state (singleton) VALUES (false)")
