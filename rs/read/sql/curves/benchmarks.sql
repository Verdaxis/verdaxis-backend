SELECT b.market_product, b.delivery_point_id::text, b.availability_window,
       b.price_per_mt_usd::text AS price, b.source,
       coalesce(b.updated_at, b.created_at) AS observed_at
FROM benchmarks b
WHERE EXISTS (
    SELECT 1 FROM jsonb_to_recordset($1::jsonb) AS k(market_product text, delivery_point_id uuid, availability_window text)
    WHERE k.market_product = b.market_product AND k.delivery_point_id = b.delivery_point_id
      AND k.availability_window = b.availability_window
)
