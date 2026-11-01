-- Test fixture for offboarding_command_postgres: one ACTIVE assisted-listing
-- authorization in organization $2 for which user $3 is accountable.
-- $4 product, $5 delivery point, $6 creating administrator, $7 support context.
INSERT INTO market_support_authorizations (
    id, organization_id, accountable_user_id, product_id, delivery_point_id,
    availability_window, quantity_mt, price_per_mt_usd, authorization_expires_at,
    certification_declared, msds_available, terms_digest, evidence_reference,
    commercial_consent_version, commercial_consent_reference, idempotency_key,
    idempotency_request_hash, created_by_actor_user_id, market_support_context_id
) VALUES (
    $1, $2, $3, $4, $5, 'SPOT', 10, 500, now() + interval '1 hour',
    false, false, repeat('a', 64), 'SYNTHETIC-EVIDENCE', 'v1', 'SYNTHETIC-CONSENT',
    'synthetic-' || $1::text, repeat('b', 64), $6, $7
)
