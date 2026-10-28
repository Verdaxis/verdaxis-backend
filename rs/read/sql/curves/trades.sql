-- Curve-trade evidence for the requested slice keys, newest REAL first.
--
-- This file is a CTE continuation of `trade_query_sql`'s `WITH eligible_trades
-- AS (...)`: it starts with a comma and adds the tail CTEs before the final
-- SELECT.
--
-- The requested key set is materialised once and matched as one composite
-- text key through `= ANY(ARRAY(SELECT ...))`, a per-row hashed lookup with no
-- join machinery. With the key set inside a per-row EXISTS, the planner chose
-- a nested loop over the key CTE and compared three columns per candidate
-- trade: measured at 65.6 million rejected comparisons and ~22 s of per-row
-- CPU on the large fixture (888 requested keys, 100k candidate trades). The
-- ANY form measured 1.8 s for the same bind. The predicate itself — which
-- trades are admitted and how they rank — is unchanged, so the row multiset
-- is unchanged; the corpus and the differential simulation prove the bytes.
, requested_curve_keys AS MATERIALIZED (
    SELECT market_product, delivery_point_id, availability_window
    FROM jsonb_to_recordset($7::jsonb)
        AS k(market_product text, delivery_point_id uuid, availability_window text)
), ranked_curve_trades AS (
    SELECT t.*, row_number() OVER (
        PARTITION BY market_product, delivery_point_id, availability_window
        ORDER BY (buyer_provenance = 'REAL') DESC, confirmed_at DESC, id DESC
    ) AS position
    FROM eligible_trades t
    WHERE (t.market_product || E'\x01' || t.delivery_point_id::text
           || E'\x01' || t.availability_window) = ANY (ARRAY(
        SELECT k.market_product || E'\x01' || k.delivery_point_id::text
               || E'\x01' || k.availability_window
        FROM requested_curve_keys AS k))
)
SELECT market_product, delivery_point_id::text, availability_window,
       price_per_mt_usd::text AS price, quantity_mt::text AS quantity,
       confirmed_at, buyer_provenance::text AS provenance
FROM ranked_curve_trades
WHERE position <= 8
ORDER BY (buyer_provenance = 'REAL') DESC, confirmed_at DESC, id DESC
