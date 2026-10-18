SELECT DISTINCT CASE $14::text
    WHEN 'products' THEN product_name
    WHEN 'regions' THEN delivery_point_region
    WHEN 'fuel-types' THEN product_fuel_type
END AS value
FROM eligible_orders
ORDER BY value
