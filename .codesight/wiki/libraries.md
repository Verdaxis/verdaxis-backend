# Libraries

> **Navigation aid.** Library inventory extracted via AST. Read the source files listed here before modifying exported functions.

**66 library files** across 2 modules

## Alembic (49 files)

- `alembic/env.py` — run_migrations_offline, do_run_migrations, run_migrations_online, run_async_migrations
- `alembic/versions/148fa1ffb191_add_notifications_table.py` — upgrade, downgrade
- `alembic/versions/14b159f248ac_rename_quote_request_to_direct_order.py` — upgrade, downgrade
- `alembic/versions/377c3f6f9aef_initial_schema.py` — upgrade, downgrade
- `alembic/versions/411b38418aea_add_user_interaction_status.py` — upgrade, downgrade
- `alembic/versions/63cbf0775d4d_add_domain_to_organization.py` — upgrade, downgrade
- `alembic/versions/a1b2c3d4e5f6_unified_orderbook_migration.py` — upgrade, downgrade
- `alembic/versions/alerts_2026_03_add_price_alerts.py` — upgrade, downgrade
- `alembic/versions/anon_trade_2026_03_add_trade_is_anonymous.py` — upgrade, downgrade
- `alembic/versions/auth_2026_07_add_must_change_password.py` — upgrade, downgrade
- `alembic/versions/b1c2d3e4f5g6_add_ci_fields_to_orderbook.py` — upgrade, downgrade
- `alembic/versions/bm_2026_04_benchmarks.py` — upgrade, downgrade
- `alembic/versions/c2d3e4f5g6h7_add_match_suggestions.py` — upgrade, downgrade
- `alembic/versions/c315709743c2_move_supplier_tier_to_organization.py` — upgrade, downgrade
- `alembic/versions/c31aa85e9739_make_user_role_nullable.py` — upgrade, downgrade
- `alembic/versions/c5e4af769a3a_add_requested_quantity_and_delivery_.py` — upgrade, downgrade
- `alembic/versions/c820b25c0e18_update_supplier_tier_values_to_match_.py` — upgrade, downgrade
- `alembic/versions/c8a8c986fb8c_add_quoteoffer_model.py` — upgrade, downgrade
- `alembic/versions/catalog_2026_03_add_product_and_delivery_point.py` — upgrade, downgrade
- `alembic/versions/catalog_2026_04_green_fuels_market_products.py` — upgrade, downgrade
- `alembic/versions/ccef95a9c9a0_add_rfq_models.py` — upgrade, downgrade
- `alembic/versions/contracts_2026_03_add_contract_supply_demand.py` — upgrade, downgrade
- `alembic/versions/d3e4f5g6h7i8_add_producer_projects.py` — upgrade, downgrade
- `alembic/versions/e3b25629c526_make_supplier_tier_nullable_and_cleanup_.py` — upgrade, downgrade
- `alembic/versions/e4f5g6h7i8j9_widen_notification_type_column.py` — upgrade, downgrade
- _…and 24 more files_

## Scripts (17 files)

- `scripts/seed_port_inventory.py` — get_region, get_price_per_mt, get_methanol_price_avg, get_biofuel_price_avg, get_stock_levels, is_certified, …
- `scripts/seed_trade_tape.py` — get_connection, clean_existing_trades, generate_trade_dates, get_status_for_date, get_price_with_drift, select_product, …
- `scripts/smoke_umami_product_analytics.py` — validate_event_data_events, validate_event_data_properties, validate_event_data_values, validate_event_data_pivot_envelope, ensure_permitted_base_url, main, …
- `scripts/ingest_market_signals.py` — assert_staging_runtime, assert_expected_database, read_rows, print_report, main, run
- `scripts/scrape_fleet_demand.py` — run_batch, extract_last_value, get_afi_article_url, get_page_body, extract_int, scrape
- `scripts/seed_vessels_fleet.py` — get_db_connection, generate_vessel_data, format_geography_point, insert_vessels, main
- `scripts/seed_realistic_orderbook.py` — rand_quantity, rand_price_around, random_created_at, build_order
- `scripts/prune_product_analytics.py` — compute_cutoff, prune_login_days, main
- `scripts/explain_product_analytics.py` — main, run
- `scripts/seed.py` — parse_args, main
- `scripts/seed_forward_monitoring_demo.py` — assert_catalog_ready, main
- `scripts/benchmark_product_analytics.py` — main
- `scripts/check_users.py` — main
- `scripts/import_gena_csv.py` — import_csv
- `scripts/run_demo_activity.py` — main
- `scripts/seed_compliance_data.py` — add_entry
- `scripts/seed_maersk_vessels.py` — seed_vessels

---
_Back to [overview.md](./overview.md)_