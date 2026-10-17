-- Shared immutable snapshot core. Parameters $1..$6 match TradeEvidenceFilters.
-- The policy relations are MATERIALIZED: inlined, the planner treats the
-- `jsonb_to_recordset` function scan as a per-row nested-loop inner and
-- re-executes it for every candidate trade (measured at ~100k re-executions
-- on the large fixture). Materialised, each policy array is read once and
-- the EXISTS predicates evaluate as hash semi-joins. The predicate set is
-- unchanged; the row multiset is unchanged.
WITH policy_products AS MATERIALIZED (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        specification_standard text,
        display_order integer
    )
),
policy_points AS MATERIALIZED (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
), formal_trades AS (
SELECT t.*
FROM trades AS t
WHERE t.market_snapshot_version = 1
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = t.product_id
          AND pp.name = t.product_name
          AND pp.fuel_type = t.fuel_type
          AND pp.fuel_grade = t.fuel_grade
          AND pp.code = t.market_product
    )
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = t.delivery_point_id
          AND pq.name = t.delivery_point_name
          AND pq.region = t.delivery_point_region
    )
    AND (
        t.availability_window = 'SPOT'
        OR t.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR t.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR t.availability_window ~ '^[0-9]{4}-CAL$'
    )
    AND t.status IN ('CONFIRMED', 'DELIVERED', 'PAID')
    AND t.confirmed_at IS NOT NULL
    AND ($2::timestamptz IS NULL OR t.confirmed_at >= $2::timestamptz)
    AND ($3::timestamptz IS NULL OR t.confirmed_at < $3::timestamptz)
    AND ($4::uuid IS NULL OR t.product_id = $4::uuid)
    AND ($5::uuid IS NULL OR t.delivery_point_id = $5::uuid)
    AND ($6::text IS NULL OR t.availability_window = $6::text)
)
