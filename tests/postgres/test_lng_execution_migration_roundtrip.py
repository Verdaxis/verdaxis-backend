"""Bio-LNG upgrade, immutable snapshots and data-preserving downgrade."""
import json

import pytest

from tests.postgres.test_supplier_offer_migration_roundtrip import supplier_scratch_database
from tests.unit.test_lng_order import ASK, BID

_PARENT = "rcp_20261003_command_results"
_REVISION = "blng_20261014_staging"
_PRODUCT = "8f6f34a5-2927-54a6-8564-cca40ae7cb8a"
_ROTTERDAM = "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"


def test_lng_migration_upgrade_constraints_and_safe_downgrade(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", _PARENT)
    previous_guard = sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)")
    migrate("upgrade", _REVISION)
    assert sql(f"SELECT name || '/' || fuel_type || '/' || fuel_grade FROM products WHERE id='{_PRODUCT}'") == "Bio-LNG/LNG/Bio"
    assert sql("SELECT count(*) FROM information_schema.columns WHERE table_schema='public' AND column_name IN ('lng_terms','lng_terms_snapshot')") == "5"
    migrate("downgrade", _PARENT)
    assert sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)") == previous_guard
    migrate("upgrade", _REVISION)
    sql(f"""
        INSERT INTO organizations (id, name, type, provenance) VALUES
          ('88888888-1111-0000-0000-000000000001', 'LNG seller', 'FUEL_TRADER', 'REAL'),
          ('88888888-1111-0000-0000-000000000002', 'LNG buyer', 'FUEL_BUYER', 'REAL');
        INSERT INTO delivery_points (id,name,region,timezone,is_active) VALUES
          ('{_ROTTERDAM}','Rotterdam','Europe','Europe/Amsterdam',true)
          ON CONFLICT DO NOTHING;
    """)
    insert_order = f"""
        INSERT INTO orderbook_orders (id, organization_id, side, product_id, delivery_point_id,
          quantity_mt, remaining_quantity_mt, price_per_mt_usd, availability_window,status,provenance,lng_terms)
        VALUES ('88888888-1111-0000-0000-000000000003','88888888-1111-0000-0000-000000000001',
          'ASK','{_PRODUCT}','{_ROTTERDAM}',10,10,1000,'SPOT','CANCELLED','REAL','{json.dumps(ASK)}');
    """
    with pytest.raises(AssertionError, match="lng_terms"):
        sql(insert_order.replace(json.dumps(ASK), json.dumps(BID)))
    sql(insert_order)
    refusal = migrate("downgrade", _PARENT, success=False)
    assert "Cannot remove Bio-LNG fields" in refusal
    assert sql("SELECT version_num FROM alembic_version") == _REVISION
    snapshot = json.dumps({"bid": BID, "ask": ASK})
    sql(f"""
        INSERT INTO trades (id,buyer_id,seller_id,initiator_org_id,initiated_by,
          buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,
          market_product,delivery_point_id,delivery_point_name,delivery_point_region,
          availability_window,quantity_mt,price_per_mt_usd,status,commission_rate_pct,lng_terms_snapshot)
        VALUES ('88888888-1111-0000-0000-000000000004',
          '88888888-1111-0000-0000-000000000002','88888888-1111-0000-0000-000000000001',
          '88888888-1111-0000-0000-000000000002','BUYER','REAL','REAL',
          '{_PRODUCT}','Bio-LNG','LNG','Bio','BIO_LNG','{_ROTTERDAM}',
          'Rotterdam','Europe','SPOT',1,1000,'PENDING_CONFIRMATION',0,'{snapshot}');
    """)
    with pytest.raises(AssertionError, match="immutable"):
        sql("UPDATE trades SET lng_terms_snapshot = jsonb_set(lng_terms_snapshot::jsonb,'{ask,bio_share_pct}','90')::json")
    sql("DELETE FROM trades; DELETE FROM orderbook_orders;")
    sql(f"""
        INSERT INTO market_indications (id,market_product,delivery_point_id,availability_window,
          side,price_per_mt_usd,source,is_demo,is_verified_real,observed_at)
        VALUES ('88888888-1111-0000-0000-000000000005','BIO_LNG','{_ROTTERDAM}',
          'SPOT','ASK',1000,'migration-test',true,false,now());
    """)
    migrate("downgrade", _PARENT, success=False)
    sql("DELETE FROM market_indications")
    migrate("downgrade", _PARENT)
    assert sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)") == previous_guard
