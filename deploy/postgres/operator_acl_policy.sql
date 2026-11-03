-- Declarative operator-role ACL policy (Phase 10 Task 21).
-- The operator role runs trusted market-signal ingestion (`ops ingest signals`)
-- and nothing else. It reads the catalog it resolves against, owns the
-- ingestion run records, and appends signal rows. Every other table, and every
-- column grant, stays denied; it never receives an app, migrator or backup
-- privilege. Signal tables have UUID keys, so there is no sequence grant.

CREATE TEMP TABLE operator_table_policy (
    table_name text PRIMARY KEY,
    privileges text[] NOT NULL,
    CHECK (privileges <@ ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']::text[])
);

INSERT INTO operator_table_policy (table_name, privileges) VALUES
    ('delivery_points', ARRAY['SELECT']),
    ('fair_price_bands', ARRAY['SELECT', 'INSERT']),
    ('market_indications', ARRAY['SELECT', 'INSERT']),
    ('market_signal_ingestion_runs', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
    ('physical_stems', ARRAY['SELECT', 'INSERT']),
    ('ports', ARRAY['SELECT']),
    ('products', ARRAY['SELECT']);
