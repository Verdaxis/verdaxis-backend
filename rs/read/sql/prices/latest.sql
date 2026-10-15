SELECT * FROM (
SELECT product_id, market_product, delivery_point_id, availability_window,
price_per_mt_usd::text AS last_price, confirmed_at AS observed_at,
row_number() OVER (PARTITION BY product_id, market_product, delivery_point_id, availability_window
ORDER BY confirmed_at DESC) AS rn
FROM eligible_trades WHERE ($7::text IS NULL OR fuel_type ILIKE '%' || $7 || '%')
AND ($8::text IS NULL OR delivery_point_region ILIKE '%' || $8 || '%')
AND ($9::text IS NULL OR market_product = $9) AND buyer_provenance = $10
) AS ranked WHERE rn = 1
