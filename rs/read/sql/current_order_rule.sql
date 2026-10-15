-- Shared expiry, owner admission and shallow declaration rule.
-- Requires policy_products and alias o. Binds $2, $3 and $13.
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
                        AND CASE WHEN $13::boolean THEN (
                            (member.kyc_status = 'APPROVED'
                             AND member.kyc_organization_id = member.organization_id)
                            OR EXISTS (
                                SELECT 1 FROM kyc_execution_grace AS grace
                                WHERE grace.user_id = member.id
                                  AND grace.organization_id = member.organization_id
                                  AND grace.expires_at > $2::timestamptz
                            )
                        ) ELSE (
                            member.kyc_organization_id IS NULL
                            OR member.kyc_organization_id = member.organization_id
                        ) END
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
                      AND CASE WHEN $13::boolean THEN (
                          (owner.kyc_status = 'APPROVED'
                           AND owner.kyc_organization_id = owner.organization_id)
                          OR EXISTS (
                              SELECT 1 FROM kyc_execution_grace AS grace
                              WHERE grace.user_id = owner.id
                                AND grace.organization_id = owner.organization_id
                                AND grace.expires_at > $2::timestamptz
                          )
                      ) ELSE (
                          owner.kyc_organization_id IS NULL
                          OR owner.kyc_organization_id = owner.organization_id
                      ) END
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
