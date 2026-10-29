-- One canonical lane and its optional quote share one statement snapshot.
-- $1 policy JSON, $2 product code, $3 point UUID, $4 canonical window.
WITH policy_products AS (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'products')
        AS p(id uuid, code text, name text, fuel_type text, fuel_grade text, lanes jsonb)
), policy_points AS (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'points')
        AS p(id uuid, name text, region text)
), catalog AS MATERIALIZED (
    SELECT d.name
    FROM policy_products pp
    JOIN products p ON p.id = pp.id AND p.name = pp.name
        AND p.fuel_type = pp.fuel_type AND p.fuel_grade = pp.fuel_grade AND p.is_active
    JOIN policy_points pd ON pd.id = $3 AND pp.lanes @> jsonb_build_array(pd.id::text)
    JOIN delivery_points d ON d.id = pd.id AND d.name = pd.name
        AND d.region = pd.region AND d.is_active
    WHERE pp.code = $2
    LIMIT 2
), quote AS MATERIALIZED (
    SELECT b.price_per_mt_usd::text AS price, b.source,
        coalesce(b.updated_at, b.created_at) AS observed_at
    FROM benchmarks b
    WHERE b.market_product = $2 AND b.delivery_point_id = $3
        AND b.availability_window = $4 AND EXISTS (SELECT 1 FROM catalog)
    LIMIT 2
)
SELECT (SELECT count(*) FROM catalog) AS catalog_count,
    (SELECT count(*) FROM quote) AS quote_count,
    (SELECT name FROM catalog LIMIT 1) AS point_name,
    (SELECT price FROM quote LIMIT 1) AS price,
    (SELECT source FROM quote LIMIT 1) AS source,
    (SELECT observed_at FROM quote LIMIT 1) AS observed_at
