-- Select the whole slice's evidence class before ranking ten levels per side.
, curve_levels AS (
    SELECT side, provenance, price_per_mt_usd,
           sum(remaining_quantity_mt) AS quantity, count(*)::bigint AS order_count
    FROM eligible_orders
    WHERE product_id = $14::uuid AND delivery_point_id = $15::uuid
      AND availability_window = $16::text
    GROUP BY side, provenance, price_per_mt_usd
), selected_curve_levels AS (
    SELECT * FROM curve_levels
    WHERE provenance = CASE WHEN EXISTS (
        SELECT 1 FROM curve_levels WHERE provenance = 'REAL'
    ) THEN 'REAL' ELSE 'DEMO' END
), ranked_curve_levels AS (
    SELECT *, row_number() OVER (
        PARTITION BY side
        ORDER BY CASE WHEN side = 'BID' THEN price_per_mt_usd END DESC,
                 CASE WHEN side = 'ASK' THEN price_per_mt_usd END ASC
    ) AS position
    FROM selected_curve_levels
)
SELECT side::text, provenance::text, price_per_mt_usd::text AS price,
       quantity::text, order_count
FROM ranked_curve_levels
WHERE position <= 10
ORDER BY side, position
