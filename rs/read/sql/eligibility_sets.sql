-- Rust decision 25: eligibility sets, computed once per statement.
--
-- The owner/actor/organization/KYC-grace checks would otherwise sit inside a
-- `CASE ... THEN EXISTS ... ELSE ... OR EXISTS ...`, which PostgreSQL cannot
-- flatten into a semi-join, so it runs a correlated subplan for every candidate
-- row. Owners and organizations are few, so each set is built once with
-- `MATERIALIZED`, and `current_order_rule.sql` tests membership instead.
--
-- Leading-comma fragment: append it to a `WITH` clause that already defines
-- `policy_products`. Binds: `$2` captured instant (shared grace expiry) and the
-- runtime gate placeholder `/*gate*/`. No other parameter.
, eligible_owners AS MATERIALIZED (
    SELECT u.id AS user_id, u.organization_id
    FROM users AS u
    JOIN organizations AS org
      ON org.id = u.organization_id
     AND org.verification_status = 'APPROVED'
    WHERE u.role IN ('BUYER', 'SUPPLIER')
      AND u.status = 'APPROVED'
      AND u.email_verified
      AND NOT u.must_change_password
      AND u.kyc_status <> 'REJECTED'
      AND CASE WHEN /*gate*/::boolean THEN (
          (u.kyc_status = 'APPROVED'
           AND u.kyc_organization_id = u.organization_id)
          OR EXISTS (
              SELECT 1 FROM kyc_execution_grace AS grace
              WHERE grace.user_id = u.id
                AND grace.organization_id = u.organization_id
                AND grace.expires_at > $2::timestamptz
          )
      ) ELSE (
          u.kyc_organization_id IS NULL
          OR u.kyc_organization_id = u.organization_id
      ) END
),
eligible_assisted AS MATERIALIZED (
    -- The member and organization checks key on the ORDER's organization, not
    -- the actor's, so an administrator is paired with each qualifying
    -- organization. Both sides are small.
    SELECT a.id AS owner_user_id, a.id AS actor_user_id, org.id AS organization_id
    FROM users AS a
    CROSS JOIN organizations AS org
    WHERE a.role = 'ADMIN'
      AND a.status = 'APPROVED'
      AND org.verification_status = 'APPROVED'
      AND org.provenance = 'REAL'
      AND EXISTS (
          SELECT 1 FROM users AS m
          WHERE m.organization_id = org.id
            AND m.role IN ('BUYER', 'SUPPLIER')
            AND m.status = 'APPROVED'
            AND m.email_verified
            AND NOT m.must_change_password
            AND m.kyc_status <> 'REJECTED'
            AND CASE WHEN /*gate*/::boolean THEN (
                (m.kyc_status = 'APPROVED'
                 AND m.kyc_organization_id = m.organization_id)
                OR EXISTS (
                    SELECT 1 FROM kyc_execution_grace AS grace
                    WHERE grace.user_id = m.id
                      AND grace.organization_id = m.organization_id
                      AND grace.expires_at > $2::timestamptz
                )
            ) ELSE (
                m.kyc_organization_id IS NULL
                OR m.kyc_organization_id = m.organization_id
            ) END
      )
)
