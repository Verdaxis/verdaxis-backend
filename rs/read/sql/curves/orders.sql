-- Aggregate every eligible order. Never limit raw liquidity rows.
SELECT product_id::text, delivery_point_id::text, availability_window,
       side::text, provenance::text,
       (CASE WHEN side = 'BID' THEN max(price_per_mt_usd)
             ELSE min(price_per_mt_usd) END)::text AS price,
       sum(remaining_quantity_mt)::text AS quantity,
       count(*)::bigint AS order_count,
       max(coalesce(updated_at, created_at)) AS observed_at,
       count(*) FILTER (WHERE idempotency_operation = 'DEMO_COVERAGE')::bigint AS managed_count
FROM eligible_orders o
WHERE (o.product_id, o.delivery_point_id, o.availability_window) IN (
    SELECT k.product_id, k.delivery_point_id, k.availability_window
    FROM jsonb_to_recordset($14::jsonb) AS k(product_id uuid, delivery_point_id uuid, availability_window text)
    WHERE k.availability_window IS NOT NULL
) OR (o.product_id, o.delivery_point_id) IN (
    SELECT k.product_id, k.delivery_point_id
    FROM jsonb_to_recordset($14::jsonb) AS k(product_id uuid, delivery_point_id uuid, availability_window text)
    WHERE k.availability_window IS NULL
)
GROUP BY product_id, delivery_point_id, availability_window, side, provenance
