, ranked_curve_trades AS (
    SELECT t.*, row_number() OVER (
        PARTITION BY market_product, delivery_point_id, availability_window
        ORDER BY (buyer_provenance = 'REAL') DESC, confirmed_at DESC, id DESC
    ) AS position
    FROM eligible_trades t
    WHERE EXISTS (
        SELECT 1 FROM jsonb_to_recordset($7::jsonb)
            AS k(market_product text, delivery_point_id uuid, availability_window text)
        WHERE k.market_product = t.market_product
          AND k.delivery_point_id = t.delivery_point_id
          AND k.availability_window = t.availability_window
    )
)
SELECT market_product, delivery_point_id::text, availability_window,
       price_per_mt_usd::text AS price, quantity_mt::text AS quantity,
       confirmed_at, buyer_provenance::text AS provenance
FROM ranked_curve_trades
WHERE position <= 8
ORDER BY (buyer_provenance = 'REAL') DESC, confirmed_at DESC, id DESC
