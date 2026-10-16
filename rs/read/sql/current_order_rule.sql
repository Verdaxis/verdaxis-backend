-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products, eligible_owners, eligible_assisted and alias o.
-- Binds $2, $3 and the gate placeholder substituted by the compositor.
TRUE
    -- Currency: an explicit future expiry, or a null expiry on REAL/UNKNOWN.
    AND (
        o.expires_at > $2::timestamptz
        OR (
            o.expires_at IS NULL
            AND o.provenance IN ('REAL', 'UNKNOWN')
        )
    )
    -- Owner admission. Assisted rows use the narrower administrator rule.
    -- Membership tests against the shared eligibility sets (decision 25).
    AND (
        CASE
            WHEN o.creation_method = 'MARKET_SUPPORT' THEN (
                (o.owner_user_id, o.created_by_actor_user_id, o.organization_id)
                    IN (SELECT owner_user_id, actor_user_id, organization_id
                        FROM eligible_assisted)
            )
            ELSE (
                o.owner_user_id IS NULL
                OR (o.owner_user_id, o.organization_id)
                    IN (SELECT user_id, organization_id FROM eligible_owners)
            )
        END
    )
    -- fame_v1 declaration currency, keyed by policy profile.
    AND (
        NOT EXISTS (
            SELECT 1
            FROM policy_products AS pf
            WHERE pf.qualification_profile = 'fame_v1'
              AND pf.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pf
                WHERE pf.qualification_profile = 'fame_v1'
                  AND pf.id = o.product_id
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
              AND pl.id = o.product_id
        )
        OR (
            EXISTS (
                SELECT 1
                FROM policy_products AS pl
                WHERE pl.qualification_profile = 'lng_v1'
                  AND pl.id = o.product_id
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
