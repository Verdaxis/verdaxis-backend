"""Change-only generation statements on an independently migrated database."""
import ast
from pathlib import Path

import pytest

from tests.postgres.test_supplier_offer_migration_roundtrip import (
    supplier_scratch_database,
)

ROOT = Path(__file__).resolve().parents[2]
REVISION = "prc_20261026_changes"
PREVIOUS = "prc_20261018_cache"
RETENTION = 4096

# Independent keys keep every operation confined to this disposable fixture.
ORG = "11111111-1111-4111-8111-111111111111"
USER = "22222222-2222-4222-8222-222222222222"
ROW = "33333333-3333-4333-8333-333333333333"
POINT = "73835e92-820e-584b-8280-bb61c63aa28e"
PRODUCT = "9510c713-6e39-5080-add3-0a7c29657b79"

INSERTS = {
    "products": "(id,name,fuel_type,fuel_grade) VALUES ('{row}','Cache fixture','Methanol','Bio')",
    "delivery_points": "(id,name,region) VALUES ('{row}','Cache fixture','Asia')",
    "organizations": "(id,name,type) VALUES ('{row}','Cache fixture','SHIPOWNER')",
    "users": "(id,email,password_hash,role,organization_id) VALUES ('{row}','cache@example.test','unused','BUYER','{org}')",
    "orderbook_orders": "(id,organization_id,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd) VALUES ('{row}','{org}','BID','{product}','{point}',200,200,600)",
    "trades": "(id,buyer_id,seller_id,initiator_org_id,initiated_by,quantity_mt,price_per_mt_usd,market_snapshot_version) VALUES ('{row}','{org}','66666666-6666-4666-8666-666666666666','{org}','BUYER',200,600,NULL)",
    "kyc_execution_grace": "(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES ('{row}','{user}','{org}','PENDING',now(),now()+interval '1 day','cache fixture')",
    "market_row_quarantines": "(id,source_table,source_id,original_row,dependencies,environment,database_name,reason,operator,reference) VALUES ('{row}','orders','{row}','{{}}','[]','test',current_database(),'fixture','fixture','fixture')",
    "organization_market_approvals": "(organization_id,previous_verification_status,reviewed_snapshot,environment,database_name,reason,operator,reference) VALUES ('{org}','PENDING','{{}}','test',current_database(),'fixture','fixture','fixture')",
    "market_signal_ingestion_runs": "(id,signal_family,source,source_kind) VALUES ('{row}','MARKET_INDICATION','fixture','MARKET_INDICATION')",
    "market_indications": "(id,market_product,delivery_point_id,availability_window,side,price_per_mt_usd,source,observed_at) VALUES ('{row}','BIO_METHANOL','{point}','SPOT','BID',600,'fixture',now())",
    "fair_price_bands": "(id,market_product,delivery_point_id,availability_window,low_price_per_mt_usd,mid_price_per_mt_usd,high_price_per_mt_usd,model_name,source,observed_at) VALUES ('{row}','BIO_METHANOL','{point}','SPOT',500,600,700,'fixture','fixture',now())",
    "physical_stems": "(id,market_product,delivery_point_id,availability_window,quantity_mt,status,source,stem_uid,observed_at) VALUES ('{row}','BIO_METHANOL','{point}','SPOT',200,'AVAILABLE','fixture','fixture',now())",
    "benchmarks": "(id,market_product,delivery_point_id,availability_window,price_per_mt_usd,source) VALUES ('{row}','BIO_METHANOL','{point}','SPOT',600,'manual_override')",
    "live_slice_benchmarks": "(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source) VALUES ('{row}','BID','BIO_METHANOL','{point}','SPOT',600,200,1,'live_slice_vwap')",
    "market_event_outbox": "(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload) VALUES ('{row}','cache_test','order','fixture','[\"{org}\"]','{{}}')",
}


@pytest.fixture(scope="module")
def cache_database():
    # Reuse the existing isolated-database producer without widening its scope.
    producer = supplier_scratch_database.__wrapped__()
    sql, migrate = next(producer)
    try:
        migrate("upgrade", PREVIOUS)
        sql("UPDATE public_read_cache_generations SET generation=5000; "
            "INSERT INTO public_read_cache_events SELECT 'trading', n FROM generate_series(1,5000) n")
        before = sql("SELECT tableoid::oid, database_epoch, primary_identity, authority_revision, active FROM public_read_cache_incarnation")
        tables = sql("SELECT oid FROM pg_class WHERE relname IN ('public_read_cache_generations','public_read_cache_events','public_read_cache_incarnation') ORDER BY oid")
        migrate("upgrade", REVISION)
        assert sql("SELECT generation FROM public_read_cache_generations WHERE domain='trading'") == "5000"
        assert sql("SELECT tableoid::oid, database_epoch, primary_identity, authority_revision, active FROM public_read_cache_incarnation") == before
        assert sql("SELECT oid FROM pg_class WHERE relname IN ('public_read_cache_generations','public_read_cache_events','public_read_cache_incarnation') ORDER BY oid") == tables
        assert sql("SELECT count(*) FROM public_read_cache_events") == str(RETENTION)
        sql(f"SET session_replication_role=replica; INSERT INTO organizations(id,name,type) VALUES ('{ORG}','Cache parent','SHIPOWNER'), ('66666666-6666-4666-8666-666666666666','Cache counterparty','FUEL_SUPPLIER'); INSERT INTO users(id,email,password_hash,role,organization_id) VALUES ('{USER}','parent@example.test','unused','BUYER','{ORG}')")
        yield sql, migrate
    finally:
        producer.close()


def snapshot_sql():
    return "SELECT generation, (SELECT count(*) FROM public_read_cache_events) FROM public_read_cache_generations WHERE domain='trading'"


@pytest.mark.parametrize("table", INSERTS)
@pytest.mark.parametrize("row_count", [1, 2])
def test_nonempty_dependency_statements_bump_once(cache_database, table, row_count):
    sql, _ = cache_database
    key = "organization_id" if table == "organization_market_approvals" else "id"
    insert = INSERTS[table].format(row=ROW, org=ORG, user=USER, point=POINT, product=PRODUCT)
    if row_count == 2:
        # Two rows in one INSERT prove the generation is statement-owned.
        second_org = "44444444-4444-4444-8444-444444444444"
        if table in {"organization_market_approvals", "kyc_execution_grace"}:
            setup = f"INSERT INTO organizations(id,name,type) VALUES ('{second_org}','Second parent','SHIPOWNER'); "
        else:
            setup = ""
        second = INSERTS[table].format(row="55555555-5555-4555-8555-555555555555", org=second_org if table in {"organization_market_approvals", "kyc_execution_grace"} else ORG, user=USER, point=POINT, product=PRODUCT)
        # Unique descriptive fields are independent too.
        second = second.replace("Cache fixture", "Second fixture").replace("cache@example.test", "second@example.test").replace("'SPOT'", "'2027-01'")
        insert += "," + second.split(" VALUES ", 1)[1]
    else:
        setup = ""
    keys = [ORG if table == "organization_market_approvals" else ROW]
    if row_count == 2:
        keys.append(second_org if table == "organization_market_approvals" else "55555555-5555-4555-8555-555555555555")
    predicate = f'"{key}" IN (' + ",".join(f"'{value}'" for value in keys) + ")"
    statements = [f'INSERT INTO public."{table}" {insert}',
                  f'UPDATE public."{table}" SET "{key}"="{key}" WHERE {predicate}',
                  f'DELETE FROM public."{table}" WHERE {predicate}']
    # Replica mode skips unrelated row guards in this synthetic fixture. Cache
    # triggers must still fire because they are ENABLE ALWAYS.
    body = "BEGIN; SET LOCAL session_replication_role=replica; " + setup
    body += "CREATE TEMP TABLE cache_before AS " + snapshot_sql() + "; "
    for index, statement in enumerate(statements, 1):
        body += statement + "; "
        body += f"DO $$ BEGIN ASSERT (SELECT generation FROM public_read_cache_generations WHERE domain='trading') = (SELECT generation+{index} FROM cache_before), 'one generation per statement'; END $$; "
    body += "ROLLBACK"
    sql(body)


def test_retention_is_a_contiguous_generation_window(cache_database):
    sql, _ = cache_database
    result = sql("BEGIN; DO $$ BEGIN FOR i IN 1..4100 LOOP UPDATE products SET is_active=is_active; END LOOP; END $$; "
                 "SELECT count(*), max(generation)-min(generation)+1 FROM public_read_cache_events WHERE domain='trading'; ROLLBACK")
    assert f"{RETENTION}|{RETENTION}" in result


def test_exact_sequencer_statement_counts_only_assigned_rows(cache_database):
    sql, _ = cache_database
    module = ast.parse((ROOT / "app/services/market_event_dispatch.py").read_text())
    statement = next(ast.literal_eval(node.value) for node in module.body
                     if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "_ASSIGN_SEQUENCES_SQL" for target in node.targets))
    statement = statement.replace("$1", "500")
    insert = INSERTS["market_event_outbox"].format(row=ROW, org=ORG)
    sql("BEGIN; DELETE FROM market_event_outbox; CREATE TEMP TABLE cache_before AS " + snapshot_sql() + "; "
        + statement + "; DO $$ BEGIN ASSERT (SELECT generation FROM public_read_cache_generations WHERE domain='trading') = (SELECT generation FROM cache_before); END $$; "
        + "INSERT INTO market_event_outbox " + insert + "; " + statement
        + "; DO $$ BEGIN ASSERT (SELECT generation FROM public_read_cache_generations WHERE domain='trading') = (SELECT generation+2 FROM cache_before); END $$; ROLLBACK")


def test_dependency_relations_support_transition_tables(cache_database):
    sql, _ = cache_database
    names = ",".join("'" + table + "'" for table in INSERTS)
    assert sql(f"SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND c.relname IN ({names}) AND (c.relkind <> 'r' OR c.relispartition OR EXISTS (SELECT 1 FROM pg_inherits WHERE inhrelid=c.oid OR inhparent=c.oid))") == "0"


def test_cache_functions_keep_migrator_ownership_and_private_execution(cache_database):
    sql, _ = cache_database
    assert sql("SELECT count(*) FROM pg_proc p JOIN pg_roles r ON r.oid=p.proowner WHERE p.oid='public.bump_public_read_cache_generation()'::regprocedure AND r.rolname='" + os.environ['RUNTIME_TEST_MIGRATOR_ROLE'] + "'") == "1"
    assert sql("SELECT has_function_privilege('" + os.environ['RUNTIME_TEST_APP_ROLE'] + "','public.bump_public_read_cache_generation()','EXECUTE')") == "f"


def test_cache_revision_requires_forward_revert(cache_database):
    _, migrate = cache_database
    assert "reviewed forward revert" in migrate("downgrade", PREVIOUS, success=False)
