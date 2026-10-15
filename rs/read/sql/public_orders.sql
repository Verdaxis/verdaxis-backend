-- Shared public-order visibility projection (Task 6).
--
-- This is the complete bound statement: every caller value is a parameter, so
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
--   $12 limit (bigint)
--   $13 offset (bigint)
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
SELECT o.id
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
    AND (
        o.availability_window = 'SPOT'
        OR o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2])$'
        OR o.availability_window ~ '^[0-9]{4}-Q[1-4]$'
        OR o.availability_window ~ '^[0-9]{4}-CAL$'
    )
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN EXISTS (
                SELECT 1
                FROM users AS actor
                WHERE actor.id = o.owner_user_id
                  AND actor.id = o.created_by_actor_user_id
                  AND actor.role = 'ADMIN'
                  AND actor.status = 'APPROVED'
                  AND EXISTS (
                      SELECT 1
                      FROM users AS member
                      WHERE member.organization_id = o.organization_id
                        AND member.role IN ('BUYER', 'SUPPLIER')
                        AND member.status = 'APPROVED'
                        AND member.email_verified
                        AND NOT member.must_change_password
                        AND member.kyc_status <> 'REJECTED'
                        AND (
                            (
                                member.kyc_status = 'APPROVED'
                                AND member.kyc_organization_id = member.organization_id
                            )
                            OR EXISTS (
                                SELECT 1
                                FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        )
                  )
                  AND EXISTS (
                      SELECT 1
                      FROM organizations AS assisted_org
                      WHERE assisted_org.id = o.organization_id
                        AND assisted_org.verification_status = 'APPROVED'
                        AND assisted_org.provenance = 'REAL'
                  )
            )
            ELSE (
                o.owner_user_id IS NULL
                OR EXISTS (
                    SELECT 1
                    FROM users AS owner
                    WHERE owner.id = o.owner_user_id
                      AND owner.organization_id = o.organization_id
                      AND owner.role IN ('BUYER', 'SUPPLIER')
                      AND owner.status = 'APPROVED'
                      AND owner.email_verified
                      AND NOT owner.must_change_password
                      AND owner.kyc_status <> 'REJECTED'
                      AND (
                          (
                              owner.kyc_status = 'APPROVED'
                              AND owner.kyc_organization_id = owner.organization_id
                          )
                          OR EXISTS (
                              SELECT 1
                              FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      )
                      AND EXISTS (
                          SELECT 1
                          FROM organizations AS org
                          WHERE org.id = o.organization_id
                            AND org.verification_status = 'APPROVED'
                      )
                )
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = p.id
              AND pf.name = p.name
              AND pf.fuel_type = p.fuel_type
              AND pf.fuel_grade = p.fuel_grade
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = p.id
                  AND pf.name = p.name
                  AND pf.fuel_type = p.fuel_type
                  AND pf.fuel_grade = p.fuel_grade
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pf.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.fame_terms ->> 'side' = o.side
            AND o.off_spec = false
            AND (
                o.side <> 'ASK'
                OR (
                    o.fame_terms ->> 'certificate_valid_until' >= $3::text
                    AND o.msds_available
                    AND o.certification_declared
                    AND length(btrim(coalesce(o.certification_scheme, ''))) > 0
                )
            )
        )
    )
    -- lng_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pl
            WHERE pl.qualification_profile = 'lng_v1'
              AND pl.id = p.id
              AND pl.name = p.name
              AND pl.fuel_type = p.fuel_type
              AND pl.fuel_grade = p.fuel_grade
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = p.id
                  AND pl.name = p.name
                  AND pl.fuel_type = p.fuel_type
                  AND pl.fuel_grade = p.fuel_grade
                  AND EXISTS (
                      SELECT 1
                      FROM jsonb_array_elements_text(pl.lanes) AS lane(value)
                      WHERE lane.value = o.delivery_point_id::text
                  )
            )
            AND o.lng_terms ->> 'side' = o.side
            AND o.off_spec = false
        )
    )
    -- Independently labelled public rows.
    AND o.provenance IN ('REAL', 'DEMO')
    -- Admitted fuel types.
    AND p.fuel_type IN ('Methanol', 'Ethanol', 'FAME', 'LNG')
    -- Scalar ASK quality declarations.
    AND (
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
    -- Off-spec rows are hidden unless the caller asks for them.
    AND (o.off_spec = false OR $4::boolean)
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
    AND ($10::text IS NULL OR dp.region = $10::text)
    AND ($11::text IS NULL OR o.availability_window = $11::text)
LIMIT $12::bigint OFFSET $13::bigint
