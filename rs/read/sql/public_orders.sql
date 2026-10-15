-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound internal relation: every caller value is a parameter, so
-- no route ever concatenates text into SQL. Parameters are
--   $1  attested market policy (jsonb text)
--   $2  captured UTC instant (timestamptz)
--   $3  Singapore date (text, YYYY-MM-DD)
--   $4  include_off_spec (boolean)
--   $5  product_id (uuid, null = any)
--   $6  delivery_point_id (uuid, null = any)
--   $7  side (text, null = any)
--   $8  fuel_type (text, null = any)
--   $9  market_product code (text, null = any)
--   $10 region (text, null = any)
--   $11 availability_window (text, null = any)
--   $12 surface: orderbook, listings, or candidates (text)
--   $13 runtime KYC execution gate enabled (boolean)
-- No pagination here: consumers rank, aggregate and page the shared relation.
--
-- Product identity comes only from the policy. The statement never names a
-- product, a product code, or a product UUID.
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
SELECT o.*, p.name AS product_name, p.fuel_type AS product_fuel_type,
       p.fuel_grade AS product_fuel_grade, p.is_active AS product_is_active,
       dp.name AS delivery_point_name, dp.region AS delivery_point_region,
       (SELECT pp.code FROM policy_products pp WHERE pp.id = p.id) AS market_product,
       -- Internal only. Never serialize this object as a public DTO.
       to_jsonb(o) || jsonb_build_object(
           'quantity_mt', o.quantity_mt::text,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'price_per_mt_usd', o.price_per_mt_usd::text,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'energy_density_mj_kg', o.energy_density_mj_kg::text
       ) AS private_digest_input
FROM orderbook_orders AS o
JOIN products AS p ON p.id = o.product_id
JOIN delivery_points AS dp ON dp.id = o.delivery_point_id
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    -- Strict canonical product identity, active row, and approved lane.
    AND p.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_products AS pp
        WHERE pp.execution_mode = 'ORDERBOOK'
          AND pp.id = p.id
          AND pp.name = p.name
          AND pp.fuel_type = p.fuel_type
          AND pp.fuel_grade = p.fuel_grade
          AND EXISTS (
              SELECT 1
              FROM jsonb_array_elements_text(pp.lanes) AS lane(value)
              WHERE lane.value = o.delivery_point_id::text
          )
    )
    -- Strict canonical delivery-point identity and active row.
    AND dp.is_active
    AND EXISTS (
        SELECT 1
        FROM policy_points AS pq
        WHERE pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    )
    -- Canonical availability-window grammar.
    AND ($12::text <> 'orderbook' OR (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    ))
    AND (/* current_order_rule */)
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND ($12::text <> 'orderbook' OR (
        o.side <> 'ASK'
        OR (
            length(btrim(coalesce(o.certification_scheme, ''))) > 0
            AND o.certification_declared
            AND length(btrim(coalesce(o.specification_standard, ''))) > 0
            AND o.msds_available
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS pd
                    WHERE pd.qualification_profile IN ('fame_v1', 'lng_v1')
                      AND pd.id = p.id
                      AND pd.name = p.name
                      AND pd.fuel_type = p.fuel_type
                      AND pd.fuel_grade = p.fuel_grade
                )
                OR o.carbon_intensity_gco2_mj IS NOT NULL
            )
            AND length(btrim(coalesce(o.feedstock, ''))) > 0
            AND (
                EXISTS (
                    SELECT 1
                    FROM policy_products AS po
                    WHERE po.qualification_profile = 'fame_v1'
                      AND po.id = p.id
                      AND po.name = p.name
                      AND po.fuel_type = p.fuel_type
                      AND po.fuel_grade = p.fuel_grade
                )
                OR length(btrim(coalesce(o.origin, ''))) > 0
            )
        )
    )
    )
    -- Off-spec rows are hidden unless the caller asks for them.
    AND ($12::text <> 'orderbook' OR o.off_spec = false OR $4::boolean)
    AND ($12::text <> 'listings' OR o.side = 'ASK')
    AND $12::text IN ('orderbook', 'listings', 'candidates')
    -- Route filters.
    AND ($5::uuid IS NULL OR o.product_id = $5::uuid)
    AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
    AND ($7::text IS NULL OR o.side = $7::text)
    AND ($8::text IS NULL OR p.fuel_type = $8::text)
    AND (
        $9::text IS NULL
        OR EXISTS (
            SELECT 1
            FROM policy_products AS pm
            WHERE pm.code = $9::text
              AND pm.id = p.id
              AND pm.name = p.name
              AND pm.fuel_type = p.fuel_type
              AND pm.fuel_grade = p.fuel_grade
        )
    )
    AND ($10::text IS NULL OR $10::text = '' OR dp.region = $10::text OR dp.name = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
