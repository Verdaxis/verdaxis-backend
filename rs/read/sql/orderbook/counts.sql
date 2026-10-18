SELECT market_product, count(*) AS order_count
FROM eligible_orders
GROUP BY market_product
