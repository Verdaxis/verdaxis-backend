SELECT s.id, s.market_product, s.delivery_point_id::text, s.availability_window,
       s.side AS partition_key, s.side, s.price_per_mt_usd::text AS price,
       s.quantity_mt::text AS quantity, s.observed_at, s.created_at,
       CASE WHEN s.is_demo THEN 'DEMO'
            WHEN s.is_verified_real AND r.verified_at IS NOT NULL AND r.source = s.source
             AND r.signal_family = 'MARKET_INDICATION' AND r.source_kind = 'MARKET_INDICATION'
            THEN 'REAL' ELSE 'UNKNOWN' END AS evidence
FROM market_indications s LEFT JOIN market_signal_ingestion_runs r ON r.id = s.trusted_ingestion_run_id
WHERE s.observed_at >= $2::timestamptz - interval '7 days'
AND /* exact_or_grid_keys */
ORDER BY s.observed_at DESC, s.created_at DESC, s.id DESC
