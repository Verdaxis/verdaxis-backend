"""B100 execution migration preserves indicative history and execution snapshots."""
from tests.postgres.test_supplier_offer_migration_roundtrip import supplier_scratch_database


_PARENT = "fame_20260922_supplier_offers"
_EXECUTION = "fame_20260922_b100_orderbook"


def test_b100_execution_rolls_back_only_without_execution_or_curve_history(supplier_scratch_database):
    sql, migrate = supplier_scratch_database
    migrate("upgrade", _PARENT)
    previous_guard = sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)")
    sql("""
        INSERT INTO organizations (id, name, type, provenance)
        VALUES ('99999999-0000-0000-0000-000000000001', 'Rollback supplier', 'FUEL_SUPPLIER', 'REAL');
        INSERT INTO users (id, email, password_hash, role, organization_id)
        VALUES ('99999999-0000-0000-0000-000000000002', 'b100-rollback@example.test',
                'unused', 'SUPPLIER', '99999999-0000-0000-0000-000000000001');
        INSERT INTO delivery_points (id, name, region, timezone, is_active)
        VALUES ('73835e92-820e-584b-8280-bb61c63aa28e', 'Singapore', 'Asia', 'Asia/Singapore', true)
        ON CONFLICT DO NOTHING;
        INSERT INTO supplier_offers (id, supplier_org_id, supplier_user_id, product_id,
            delivery_point_id, quantity_mt, min_fill_mt, price_per_mt_usd, availability_window,
            listing_terms, expires_at, created_at, updated_at)
        VALUES ('99999999-0000-0000-0000-000000000004', '99999999-0000-0000-0000-000000000001',
            '99999999-0000-0000-0000-000000000002', 'e561e43f-d9b2-598e-981c-f1d28d515ddc',
            '73835e92-820e-584b-8280-bb61c63aa28e', 100, 20, 1050, 'SPOT',
            '{"batch_reference":"unchanged-indication"}', now() + interval '1 day', now(), now());
        INSERT INTO rfqs (id, buyer_org_id, product_id, delivery_point_id, quantity_mt,
            availability_window, is_anonymous, status, expires_at, created_at, contract_terms)
        VALUES ('99999999-0000-0000-0000-000000000003', '99999999-0000-0000-0000-000000000001',
            'e561e43f-d9b2-598e-981c-f1d28d515ddc', '73835e92-820e-584b-8280-bb61c63aa28e',
            100, 'SPOT', false, 'OPEN', now() + interval '1 day', now(), '{"schema_version":1}');
    """)
    migrate("upgrade", _EXECUTION)
    # Catalog access never turns prior declarations into executable liquidity.
    assert sql("SELECT count(*) FROM orderbook_orders") == "0"
    assert sql("SELECT count(*) FROM trades") == "0"
    assert sql("SELECT listing_terms->>'batch_reference' FROM supplier_offers") == "unchanged-indication"
    assert sql("SELECT contract_terms->>'schema_version' FROM rfqs") == "1"
    migrate("downgrade", _PARENT)
    assert sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)") == previous_guard
    assert sql("SELECT listing_terms->>'batch_reference' FROM supplier_offers") == "unchanged-indication"
    migrate("upgrade", _EXECUTION)
    sql("""
        INSERT INTO orderbook_orders (id, organization_id, side, product_id, delivery_point_id,
            quantity_mt, remaining_quantity_mt, price_per_mt_usd, availability_window,
            status, provenance, fame_terms)
        VALUES ('99999999-0000-0000-0000-000000000005', '99999999-0000-0000-0000-000000000001',
            'ASK', 'e561e43f-d9b2-598e-981c-f1d28d515ddc', '73835e92-820e-584b-8280-bb61c63aa28e',
            100, 100, 1050, 'SPOT', 'CANCELLED', 'REAL', '{"schema_version":1,"side":"ASK"}');
    """)
    refusal = migrate("downgrade", _PARENT, success=False)
    assert "Cannot remove B100 execution fields while contract or market evidence history exists" in refusal
    assert sql("SELECT version_num FROM alembic_version") == _EXECUTION
    assert sql("SELECT fame_terms->>'side' FROM orderbook_orders") == "ASK"
    sql("DELETE FROM orderbook_orders")
    sql("""
        INSERT INTO market_indications (id, market_product, delivery_point_id, availability_window,
            side, price_per_mt_usd, source, is_demo, is_verified_real, observed_at)
        VALUES ('99999999-0000-0000-0000-000000000006', 'UCOME_B100',
            '73835e92-820e-584b-8280-bb61c63aa28e', 'SPOT', 'ASK', 1050,
            'rollback-test', true, false, now());
    """)
    migrate("downgrade", _PARENT, success=False)
    assert sql("SELECT version_num FROM alembic_version") == _EXECUTION
    assert sql("SELECT market_product FROM market_indications") == "UCOME_B100"
    sql("DELETE FROM market_indications")
    migrate("downgrade", _PARENT)
    assert sql("SELECT pg_get_functiondef('verdaxis_validate_trade_snapshot()'::regprocedure)") == previous_guard
    assert sql("SELECT count(*) FROM supplier_offers") == "1"
    assert sql("SELECT count(*) FROM rfqs") == "1"
