, grouped AS (
    SELECT product_id, product_name, market_product, product_fuel_type,
           delivery_point_id, delivery_point_name, delivery_point_region,
           availability_window, side, provenance,
           min(price_per_mt_usd) AS min_price,
           max(price_per_mt_usd) AS max_price,
           sum(remaining_quantity_mt) AS total_quantity,
           count(*) AS order_count, max(created_at) AS observed_at
    FROM eligible_orders
    GROUP BY product_id, product_name, market_product, product_fuel_type,
             delivery_point_id, delivery_point_name, delivery_point_region,
             availability_window, side, provenance
)
SELECT *, min_price::text AS min_price_text, max_price::text AS max_price_text,
       total_quantity::text AS total_quantity_text,
       (sum(order_count) OVER (PARTITION BY product_id, provenance))::bigint
           AS product_total_order_count
FROM grouped
ORDER BY market_product, delivery_point_name, availability_window, side, provenance
LIMIT $14
