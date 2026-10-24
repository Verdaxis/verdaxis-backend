-- Fixed fixture rows for the P02–P04 component cases (Task 17b).
--
-- Organization UUIDs encode their role in the case:
--   ...a1 Alpha Shipping   REAL, one buyer            (market member)
--   ...a2 Bravo Fuels      REAL, one supplier, one pending buyer
--   ...a3 Charlie Trading  REAL, one supplier, no orders
--   ...d1 Demo Liquidity   DEMO, no account at all
--   ...d2 Operations       REAL, administrators only
--   ...d3 Legacy Holdings  REAL, one account with a NULL role
--
-- Times are UTC. A row placed at `date_trunc('day', now())` is today's bucket,
-- so the 3-day series is [two days ago, yesterday, today] on every host.
INSERT INTO organizations (id, name, domain, type, verification_status, provenance) VALUES
    ('00000000-0000-4000-8000-0000000000a1', 'Alpha Shipping', 'alpha.test', 'SHIPPING_LINE', 'APPROVED', 'REAL'),
    ('00000000-0000-4000-8000-0000000000a2', 'Bravo Fuels', 'bravo.test', 'FUEL_SUPPLIER', 'APPROVED', 'REAL'),
    ('00000000-0000-4000-8000-0000000000a3', 'Charlie Trading', 'charlie.test', 'FUEL_TRADER', 'APPROVED', 'REAL'),
    ('00000000-0000-4000-8000-0000000000d1', 'Demo Liquidity', 'demo.test', 'FUEL_SUPPLIER', 'APPROVED', 'DEMO'),
    ('00000000-0000-4000-8000-0000000000d2', 'Operations', 'ops.test', 'OTHER', 'APPROVED', 'REAL'),
    ('00000000-0000-4000-8000-0000000000d3', 'Legacy Holdings', 'legacy.test', 'OTHER', 'APPROVED', 'REAL');

-- Newest first in the P04 listing: buyer A, supplier A, buyer B, orgless, supplier C.
INSERT INTO users (id, email, first_name, last_name, role, status, organization_id, email_verified, must_change_password, last_login, created_at) VALUES
    ('00000000-0000-4000-8000-0000000000b1', 'alpha@example.test', 'Pat', 'Lee', 'BUYER', 'APPROVED',
     '00000000-0000-4000-8000-0000000000a1', true, false, now() - interval '1 day', now() - interval '1 day'),
    ('00000000-0000-4000-8000-0000000000b2', 'supplier@example.test', 'Sam', 'Park', 'SUPPLIER', 'APPROVED',
     '00000000-0000-4000-8000-0000000000a2', true, false, now() - interval '30 days', now() - interval '2 days'),
    ('00000000-0000-4000-8000-0000000000b3', 'beta@example.test', NULL, NULL, 'BUYER', 'PENDING',
     '00000000-0000-4000-8000-0000000000a2', false, true, NULL, now() - interval '3 days'),
    ('00000000-0000-4000-8000-0000000000b6', 'gamma@example.test', 'Gil', 'Ng', 'BUYER', 'APPROVED',
     NULL, true, false, NULL, now() - interval '4 days'),
    ('00000000-0000-4000-8000-0000000000b7', 'delta@example.test', 'Dee', 'Ortiz', 'SUPPLIER', 'APPROVED',
     '00000000-0000-4000-8000-0000000000a3', true, false, NULL, now() - interval '5 days'),
    ('00000000-0000-4000-8000-0000000000b4', 'ops@example.test', 'Ola', 'Reid', 'ADMIN', 'APPROVED',
     '00000000-0000-4000-8000-0000000000d2', true, false, now() - interval '1 hour', now() - interval '6 days'),
    ('00000000-0000-4000-8000-0000000000b8', 'ops2@example.test', 'Oli', 'Rowe', 'ADMIN', 'APPROVED',
     '00000000-0000-4000-8000-0000000000d2', true, false, NULL, now() - interval '6 days'),
    ('00000000-0000-4000-8000-0000000000b5', 'legacy@example.test', 'Lee', 'Gacy', NULL, 'PENDING',
     '00000000-0000-4000-8000-0000000000d3', false, false, NULL, now() - interval '7 days');

-- Five orders from the two market organizations: three today, one yesterday,
-- one expired five days ago. Two are open (OPEN / PARTIALLY_FILLED, unexpired).
INSERT INTO orderbook_orders (id, organization_id, side, status, expires_at, created_at) VALUES
    ('00000000-0000-4000-8000-0000000000c1', '00000000-0000-4000-8000-0000000000a1', 'BID', 'OPEN', NULL,
     date_trunc('day', now()) + interval '9 hours'),
    ('00000000-0000-4000-8000-0000000000c2', '00000000-0000-4000-8000-0000000000a2', 'ASK', 'PARTIALLY_FILLED', NULL,
     date_trunc('day', now()) + interval '10 hours'),
    ('00000000-0000-4000-8000-0000000000c3', '00000000-0000-4000-8000-0000000000a1', 'ASK', 'OPEN', now() + interval '1 day',
     date_trunc('day', now()) + interval '11 hours'),
    ('00000000-0000-4000-8000-0000000000c4', '00000000-0000-4000-8000-0000000000a1', 'BID', 'FILLED', NULL,
     date_trunc('day', now() - interval '1 day') + interval '9 hours'),
    ('00000000-0000-4000-8000-0000000000c5', '00000000-0000-4000-8000-0000000000a2', 'ASK', 'OPEN', now() - interval '1 day',
     date_trunc('day', now() - interval '5 days') + interval '9 hours');

-- Four trades between market organizations. Only CONFIRMED/DELIVERED/PAID are
-- "executed"; the pending trade still counts in `total_trades`.
INSERT INTO trades (id, buyer_id, seller_id, status, quantity_mt, final_total_usd, commission_amount_usd, created_at) VALUES
    ('00000000-0000-4000-8000-0000000000e1', '00000000-0000-4000-8000-0000000000a1', '00000000-0000-4000-8000-0000000000a2',
     'PENDING_CONFIRMATION', 5, NULL, 2.5, date_trunc('day', now()) + interval '12 hours'),
    ('00000000-0000-4000-8000-0000000000e2', '00000000-0000-4000-8000-0000000000a1', '00000000-0000-4000-8000-0000000000a2',
     'CONFIRMED', 10, 1000.00, 5.00, date_trunc('day', now()) + interval '13 hours'),
    ('00000000-0000-4000-8000-0000000000e3', '00000000-0000-4000-8000-0000000000a1', '00000000-0000-4000-8000-0000000000a3',
     'DELIVERED', 20, 2000.00, 10.00, date_trunc('day', now() - interval '1 day') + interval '9 hours'),
    ('00000000-0000-4000-8000-0000000000e4', '00000000-0000-4000-8000-0000000000a1', '00000000-0000-4000-8000-0000000000a2',
     'PAID', 5, 1000.00, 5.00, date_trunc('day', now() - interval '3 days') + interval '9 hours');
