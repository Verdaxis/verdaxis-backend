SELECT count(DISTINCT id) AS count FROM eligible_trades WHERE ($7::text IS NULL OR fuel_type = $7)
AND ($8::text IS NULL OR market_product = $8)
AND ($9::text IS NULL OR delivery_point_region = $9 OR delivery_point_name = $9)
AND (NOT $10::boolean OR buyer_provenance = 'DEMO')
