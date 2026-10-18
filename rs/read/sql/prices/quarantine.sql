SELECT product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region, count(*) AS count, max(confirmed_at) AS observed_at
FROM quarantine_metadata WHERE ($7::text IS NULL OR fuel_type ILIKE '%' || $7 || '%')
AND ($8::text IS NULL OR delivery_point_region ILIKE '%' || $8 || '%')
AND ($9::text IS NULL OR market_product = $9)
GROUP BY product_id, product_name, fuel_type, fuel_grade, market_product, availability_window, delivery_point_id, delivery_point_name, delivery_point_region
