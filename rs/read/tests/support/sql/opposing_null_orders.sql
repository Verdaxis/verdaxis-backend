-- Isolated base-scan fixture. The shared public relation excludes NULL points
-- today; this fixture tests the prefilter's contract without relaxing admission.
SELECT * FROM (VALUES
    ('00000000-0000-0000-0000-000000000001'::uuid, NULL::uuid, 'SPOT', 'REAL', 'BID', 610.25::numeric(12,2)),
    ('00000000-0000-0000-0000-000000000001'::uuid, NULL::uuid, 'SPOT', 'REAL', 'ASK', 620.75::numeric(12,2)),
    ('00000000-0000-0000-0000-000000000001'::uuid, NULL::uuid, 'SPOT', 'DEMO', 'BID', 999.99::numeric(12,2)),
    ('00000000-0000-0000-0000-000000000001'::uuid, NULL::uuid, 'SPOT', 'DEMO', 'ASK', 1.01::numeric(12,2))
) AS fixture(product_id, delivery_point_id, availability_window, provenance, side, price_per_mt_usd)
