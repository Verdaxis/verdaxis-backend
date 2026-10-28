SELECT id, product_id, market_product, fuel_type, fuel_grade, delivery_point_id, delivery_point_name, delivery_point_region, availability_window, quantity_mt::text AS quantity, price_per_mt_usd::text AS price, (quantity_mt * price_per_mt_usd)::text AS total, confirmed_at, buyer_provenance
FROM eligible_trades WHERE ($7::text IS NULL OR fuel_type = $7)
AND ($8::text IS NULL OR market_product = $8)
AND ($9::text IS NULL OR delivery_point_region = $9 OR delivery_point_name = $9)
AND (NOT $10::boolean OR buyer_provenance = 'DEMO')
ORDER BY confirmed_at DESC, id DESC LIMIT $11 OFFSET $12
