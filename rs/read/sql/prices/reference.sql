SELECT product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region, date(confirmed_at) AS trade_date,
sum(price_per_mt_usd * quantity_mt)::text AS weighted_sum,
sum(quantity_mt)::text AS volume, count(*) AS count
FROM eligible_trades WHERE ($7::text IS NULL OR fuel_type ILIKE '%' || $7 || '%')
AND ($8::text IS NULL OR delivery_point_region ILIKE '%' || $8 || '%')
AND ($9::text IS NULL OR market_product = $9) AND buyer_provenance = 'REAL'
AND ($10::date IS NULL OR date(confirmed_at) >= $10)
AND ($11::date IS NULL OR date(confirmed_at) <= $11)
GROUP BY product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region, date(confirmed_at) ORDER BY trade_date DESC
