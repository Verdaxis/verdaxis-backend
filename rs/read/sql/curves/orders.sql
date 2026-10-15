SELECT product_id::text, delivery_point_id::text, availability_window,
       side::text, provenance::text, price_per_mt_usd::text AS price,
       remaining_quantity_mt::text AS quantity,
       coalesce(updated_at, created_at) AS observed_at,
       idempotency_operation = 'DEMO_COVERAGE' AS managed
FROM eligible_orders o
WHERE EXISTS (
    SELECT 1 FROM jsonb_to_recordset($14::jsonb) AS k(product_id uuid, delivery_point_id uuid, availability_window text)
    WHERE k.product_id = o.product_id AND k.delivery_point_id = o.delivery_point_id
      AND (k.availability_window IS NULL OR k.availability_window = o.availability_window)
)
