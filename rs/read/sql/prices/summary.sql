SELECT product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region, max(price_per_mt_usd)::text AS high,
min(price_per_mt_usd)::text AS low, avg(price_per_mt_usd)::text AS average,
sum(quantity_mt)::text AS volume, count(*) AS count, max(confirmed_at) AS observed_at
FROM eligible_trades WHERE ($7::text IS NULL OR fuel_type ILIKE '%' || $7 || '%')
AND ($8::text IS NULL OR delivery_point_region ILIKE '%' || $8 || '%')
AND ($9::text IS NULL OR market_product = $9) AND buyer_provenance = $10
GROUP BY product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region
