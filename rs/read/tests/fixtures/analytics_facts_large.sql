-- Sixty cohorts, four entities each, sixty weekly activity offsets.
-- Each grouped result must retain exact counts while bounding transferred rows.
INSERT INTO organizations (id, name, provenance, created_at)
SELECT md5('large-org-' || i)::uuid, 'Fixture ' || i, 'REAL',
       TIMESTAMPTZ '2025-01-01 00:00:00Z'
FROM generate_series(0, 239) i;
INSERT INTO users (id, email, password_hash, role, status, organization_id, created_at, email_verified)
SELECT md5('large-user-' || i)::uuid, 'large' || i || '@fixture.test', 'x',
       'BUYER', 'APPROVED', md5('large-org-' || i)::uuid,
       TIMESTAMPTZ '2025-01-01 00:00:00Z', true
FROM generate_series(0, 239) i;
INSERT INTO user_login_days (id, activity_date, user_id, organization_id, role, first_login_at, last_login_at)
SELECT md5('large-login-' || i || '-' || w)::uuid,
       DATE '2025-01-06' + (i / 4 + w) * 7,
       md5('large-user-' || i)::uuid, md5('large-org-' || i)::uuid, 'BUYER',
       TIMESTAMPTZ '2025-01-06 00:00:00Z' + (i / 4 + w) * INTERVAL '7 days',
       TIMESTAMPTZ '2025-01-06 00:00:00Z' + (i / 4 + w) * INTERVAL '7 days'
FROM generate_series(0, 239) i CROSS JOIN generate_series(0, 59) w;
INSERT INTO orderbook_orders (id, organization_id, side, status, provenance, created_at, remaining_quantity_mt)
SELECT md5('large-order-' || i || '-' || w)::uuid, md5('large-org-' || i)::uuid,
       'BID', 'OPEN', 'REAL',
       TIMESTAMPTZ '2025-01-06 00:00:00Z' + (i / 4 + w) * INTERVAL '7 days', 10
FROM generate_series(0, 239) i CROSS JOIN generate_series(0, 59) w;
