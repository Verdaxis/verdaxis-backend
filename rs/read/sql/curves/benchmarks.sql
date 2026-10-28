-- Persisted benchmark quotes for the requested slice keys.
--
-- `requested_curve_keys` is materialised once and matched as one composite
-- text key through `= ANY(ARRAY(SELECT ...))`, for the same measured reason as
-- in `trades.sql`: an EXISTS over the key set let the planner choose a nested
-- loop that compared three columns per benchmark row. The predicate and the
-- row multiset are unchanged.
WITH requested_curve_keys AS MATERIALIZED (
    SELECT market_product, delivery_point_id, availability_window
    FROM jsonb_to_recordset($1::jsonb)
        AS k(market_product text, delivery_point_id uuid, availability_window text)
)
SELECT b.market_product, b.delivery_point_id::text, b.availability_window,
       b.price_per_mt_usd::text AS price, b.source,
       coalesce(b.updated_at, b.created_at) AS observed_at
FROM benchmarks b
WHERE (b.market_product || E'\x01' || b.delivery_point_id::text
       || E'\x01' || b.availability_window) = ANY (ARRAY(
    SELECT k.market_product || E'\x01' || k.delivery_point_id::text
           || E'\x01' || k.availability_window
    FROM requested_curve_keys AS k))
