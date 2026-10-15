-- Formal trade evidence: immutable canonical snapshots, exact provenance.
--
-- Port of `app.services.market_provenance.public_trade_evidence_clause`. A
-- trade is evidence only when it carries a version-1 canonical snapshot, both
-- sides share one exact provenance class, and it has reached a formal status
-- with a confirmation instant. Real historical history is never rebuilt from
-- mutable order metadata.
--
-- Parameters:
--   $1 attested market policy (jsonb text)
--   $2 confirmed_since (timestamptz, null = unbounded)
--   $3 confirmed_before (timestamptz, null = unbounded)
--   $4 product_id (uuid, null = any)
--   $5 delivery_point_id (uuid, null = any)
--   $6 availability_window (text, null = any)
--   $7 limit (bigint)
--   $8 offset (bigint)
WITH policy_products AS (
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
        display_order integer
    )
),
policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT t.id
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
    AND (
        (t.buyer_provenance = 'REAL' AND t.seller_provenance = 'REAL')
        OR (t.buyer_provenance = 'DEMO' AND t.seller_provenance = 'DEMO')
    )
    AND t.status IN ('CONFIRMED', 'DELIVERED', 'PAID')
    AND t.confirmed_at IS NOT NULL
    AND ($2::timestamptz IS NULL OR t.confirmed_at >= $2::timestamptz)
    AND ($3::timestamptz IS NULL OR t.confirmed_at < $3::timestamptz)
    AND ($4::uuid IS NULL OR t.product_id = $4::uuid)
    AND ($5::uuid IS NULL OR t.delivery_point_id = $5::uuid)
    AND ($6::text IS NULL OR t.availability_window = $6::text)
LIMIT $7::bigint OFFSET $8::bigint
