, grouped AS (
    SELECT product_id, delivery_point_id, availability_window, side, provenance,
           min(price_per_mt_usd) AS min_price,
           max(price_per_mt_usd) AS max_price,
           sum(remaining_quantity_mt) AS total_quantity,
           count(*) AS order_count,
           max(created_at) AS observed_at
    FROM eligible_orders
    GROUP BY product_id, delivery_point_id, availability_window, side, provenance
), labelled AS (
    SELECT g.product_id,
           p.name AS product_name,
           (SELECT pp.code
            FROM jsonb_to_recordset(($1::jsonb) -> 'products')
                AS pp(id uuid, code text)
            WHERE pp.id = g.product_id) AS market_product,
           p.fuel_type AS product_fuel_type,
           g.delivery_point_id,
           dp.name AS delivery_point_name,
           dp.region AS delivery_point_region,
           g.availability_window, g.side, g.provenance,
           g.min_price, g.max_price, g.total_quantity,
           g.order_count, g.observed_at
    FROM grouped AS g
    JOIN products AS p ON p.id = g.product_id
    JOIN delivery_points AS dp ON dp.id = g.delivery_point_id
)
SELECT *,
       min_price::text AS min_price_text,
       max_price::text AS max_price_text,
       total_quantity::text AS total_quantity_text,
       (sum(order_count) OVER (
           PARTITION BY product_id, provenance
       ))::bigint AS product_total_order_count
FROM labelled
ORDER BY market_product, delivery_point_name, availability_window, side, provenance
LIMIT $14
