SELECT availability_window, side,
       max(price_per_mt_usd)::text AS max_price,
       min(price_per_mt_usd)::text AS min_price,
       sum(remaining_quantity_mt) AS total_volume,
       count(id) AS order_count
FROM eligible_orders
GROUP BY availability_window, side
