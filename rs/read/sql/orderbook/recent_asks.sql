, ranked AS (
    SELECT *, row_number() OVER (
        PARTITION BY delivery_point_id ORDER BY created_at DESC, id DESC
    ) AS point_rank
    FROM eligible_orders
)
SELECT *, price_per_mt_usd::text AS price_text,
       remaining_quantity_mt::text AS quantity_text
FROM ranked WHERE point_rank = 1
ORDER BY delivery_point_name
