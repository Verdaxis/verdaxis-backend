, benchmark_candidates AS MATERIALIZED (
    SELECT side, market_product, delivery_point_id, availability_window,
           remaining_quantity_mt, price_per_mt_usd, private_digest_input
    FROM eligible_orders
), requested_keys AS MATERIALIZED (
    SELECT DISTINCT side, product, point, "window"
    FROM jsonb_to_recordset($14::jsonb) AS requested(
        side text, product text, point uuid, "window" text
    )
)
SELECT o.side, o.market_product, o.delivery_point_id, o.availability_window,
       o.remaining_quantity_mt, o.price_per_mt_usd, o.private_digest_input
FROM benchmark_candidates AS o
JOIN requested_keys AS requested
    ON (o.side, o.market_product, o.delivery_point_id, o.availability_window)
        = (requested.side, requested.product, requested.point, requested."window")
