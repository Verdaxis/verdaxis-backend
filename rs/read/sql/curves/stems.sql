SELECT s.id, s.market_product, s.delivery_point_id::text, s.availability_window,
       jsonb_build_array(s.source, s.stem_uid)::text AS partition_key,
       s.status, s.quantity_mt::text AS quantity, s.stem_start, s.stem_end,
       s.observed_at, s.created_at,
       CASE WHEN s.is_demo THEN 'DEMO'
            WHEN s.is_verified_real AND r.verified_at IS NOT NULL AND r.source = s.source
             AND r.signal_family = 'PHYSICAL_STEM' AND r.source_kind = 'PHYSICAL_STEM'
            THEN 'REAL' ELSE 'UNKNOWN' END AS evidence
FROM physical_stems s LEFT JOIN market_signal_ingestion_runs r ON r.id = s.trusted_ingestion_run_id
WHERE s.observed_at >= $2::timestamptz - interval '90 days'
AND EXISTS (SELECT 1 FROM jsonb_to_recordset($1::jsonb) AS k(market_product text, delivery_point_id uuid, availability_window text)
 WHERE k.market_product = s.market_product AND k.delivery_point_id = s.delivery_point_id AND k.availability_window = s.availability_window)
ORDER BY s.observed_at DESC, s.created_at DESC, s.id DESC
