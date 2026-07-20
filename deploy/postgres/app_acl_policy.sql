-- Central declarative application-role ACL policy.
-- alembic_version and extension objects remain outside governed application
-- objects. Future seed_runs and market_row_quarantines are intentionally
-- read-only to the app when those integration-owned tables exist. Unknown
-- tables and sequences receive no app authority. Audit/status history is
-- append-only. organizations.verification_status stays absent from the INSERT
-- column policy (signup relies on the model server_default); the security
-- checkpoint added exactly one runtime write path — the admin organization
-- admission review endpoints — so it carries a single UPDATE column grant.
-- users keeps its blanket table-level DML grant, which covers the security
-- token-hash/KYC-review/admission columns; no per-column users entries exist.
-- organization_join_requests has no app DELETE path: requests are reviewed in
-- place, never removed by the app role. Undeclared columns remain denied.

CREATE TEMP TABLE app_table_policy (
    table_name text PRIMARY KEY,
    privileges text[] NOT NULL,
    CHECK (privileges <@ ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']::text[])
);

INSERT INTO app_table_policy (table_name, privileges) VALUES
    ('audit_logs', ARRAY['SELECT', 'INSERT']),
    ('benchmarks', ARRAY['SELECT']),
    ('commissions', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('compliance_ledger', ARRAY['SELECT']),
    ('contracts', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('delivery_points', ARRAY['SELECT']),
    ('demand_profiles', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('direct_order_offers', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('direct_orders', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('fair_price_bands', ARRAY['SELECT']),
    ('inventory_items', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('live_slice_benchmarks', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('market_indications', ARRAY['SELECT']),
    ('market_row_quarantines', ARRAY['SELECT']),
    ('market_signal_ingestion_runs', ARRAY['SELECT']),
    ('match_suggestions', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('negotiation_rounds', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('negotiations', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('news_items', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('notifications', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('orderbook_orders', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('orders', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('organization_join_requests', ARRAY['SELECT', 'INSERT', 'UPDATE']),
    ('organizations', ARRAY['SELECT', 'DELETE']),
    ('pending_registrations', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('physical_stems', ARRAY['SELECT']),
    ('port_intelligence', ARRAY['SELECT']),
    ('ports', ARRAY['SELECT']),
    ('price_alerts', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('producer_projects', ARRAY['SELECT']),
    ('products', ARRAY['SELECT']),
    ('public_listings', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('referrals', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('refresh_sessions', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('rfq_quotes', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('rfqs', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('seed_runs', ARRAY['SELECT']),
    ('subscriptions', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('supply_listings', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('traceability_events', ARRAY['SELECT']),
    ('trades', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('user_login_days', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('user_preferences', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('user_status_transitions', ARRAY['SELECT', 'INSERT']),
    ('users', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('vessels', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('watchlist_entries', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('watchlist_events', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('watchlist_targets', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('watchlists', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']);

CREATE TEMP TABLE app_column_policy (
    table_name text NOT NULL,
    column_name text NOT NULL,
    privilege_type text NOT NULL CHECK (privilege_type IN ('INSERT', 'UPDATE')),
    PRIMARY KEY (table_name, column_name, privilege_type),
    FOREIGN KEY (table_name) REFERENCES app_table_policy (table_name)
);

INSERT INTO app_column_policy (table_name, column_name, privilege_type) VALUES
    ('organizations', 'id', 'INSERT'),
    ('organizations', 'name', 'INSERT'),
    ('organizations', 'domain', 'INSERT'),
    ('organizations', 'type', 'INSERT'),
    ('organizations', 'supplier_tier', 'INSERT'),
    ('organizations', 'tax_id', 'INSERT'),
    ('organizations', 'country_code', 'INSERT'),
    ('organizations', 'created_at', 'INSERT'),
    ('organizations', 'name', 'UPDATE'),
    ('organizations', 'domain', 'UPDATE'),
    ('organizations', 'type', 'UPDATE'),
    ('organizations', 'supplier_tier', 'UPDATE'),
    ('organizations', 'tax_id', 'UPDATE'),
    ('organizations', 'country_code', 'UPDATE'),
    ('organizations', 'verification_status', 'UPDATE');

CREATE TEMP TABLE app_sequence_policy (
    sequence_name text PRIMARY KEY,
    privileges text[] NOT NULL,
    CHECK (privileges <@ ARRAY['USAGE', 'SELECT', 'UPDATE']::text[])
);
