SELECT DISTINCT dp.region AS value
FROM products AS p
CROSS JOIN delivery_points AS dp
CROSS JOIN LATERAL (
    SELECT 1
    FROM eligible_orders AS e
    WHERE e.product_id = p.id
      AND e.delivery_point_id = dp.id
    LIMIT 1 OFFSET 0
) AS hit
ORDER BY value
