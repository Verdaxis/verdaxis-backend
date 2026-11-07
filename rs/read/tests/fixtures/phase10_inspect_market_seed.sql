-- Market rows for the Phase 10 inspection tests. Load after
-- phase10_inspect_seed.sql, as the database owner. The counts it adds are
-- listed at the top of that file.
--
-- Organization shorthand: A1/A2 are real organizations, D1/D2 are canonical
-- demo organizations, T1/T2 are known test organizations. An exempt pair has
-- both parties in the same registry; a mixed pair (D1/T1) is not exempt.
BEGIN;

-- Orders: five without an owner (two outstanding), one owned.
INSERT INTO orderbook_orders (id, organization_id, side, quantity_mt, remaining_quantity_mt, price_per_mt_usd,
                              status, expires_at, product_id, provenance, owner_user_id) VALUES
  ('c1000000-0000-4000-8000-000000000001', 'a1000000-0000-4000-8000-000000000001', 'BID', 10, 10, 100, 'OPEN',
     NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'REAL', NULL),
  ('c2000000-0000-4000-8000-000000000002', 'a1000000-0000-4000-8000-000000000001', 'BID', 10, 4, 100, 'PARTIALLY_FILLED',
     NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'REAL', NULL),
  ('c3000000-0000-4000-8000-000000000003', 'a1000000-0000-4000-8000-000000000001', 'BID', 10, 10, 100, 'CANCELLED',
     NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'REAL', NULL),
  ('c4000000-0000-4000-8000-000000000004', '4da7b285-34ee-5443-9406-f96b4ed1a251', 'BID', 10, 10, 100, 'OPEN',
     now() + interval '1 day', 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'DEMO', NULL),
  ('c5000000-0000-4000-8000-000000000005', '9e63f7a1-0000-4000-8000-000000000001', 'BID', 10, 10, 100, 'OPEN',
     NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'TEST', NULL),
  ('c6000000-0000-4000-8000-000000000006', 'a1000000-0000-4000-8000-000000000001', 'BID', 10, 10, 100, 'OPEN',
     NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 'REAL', 'e1000000-0000-4000-8000-000000000001');

-- RFQs: five without a buyer user (two outstanding), one with a buyer user.
INSERT INTO rfqs (id, buyer_org_id, buyer_user_id, product_id, quantity_mt, expires_at, status) VALUES
  ('d1000000-0000-4000-8000-000000000001', 'a1000000-0000-4000-8000-000000000001', NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'OPEN'),
  ('d2000000-0000-4000-8000-000000000002', 'a1000000-0000-4000-8000-000000000001', NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'QUOTED'),
  ('d3000000-0000-4000-8000-000000000003', 'a1000000-0000-4000-8000-000000000001', NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'EXPIRED'),
  ('d4000000-0000-4000-8000-000000000004', '4da7b285-34ee-5443-9406-f96b4ed1a251', NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'OPEN'),
  ('d5000000-0000-4000-8000-000000000005', '9e63f7a1-0000-4000-8000-000000000001', NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'OPEN'),
  ('d6000000-0000-4000-8000-000000000006', 'a1000000-0000-4000-8000-000000000001', 'e1000000-0000-4000-8000-000000000001', 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, now() + interval '1 day', 'OPEN');

-- Quotes: five without a seller user (two outstanding), one with a seller user.
INSERT INTO rfq_quotes (id, rfq_id, seller_org_id, seller_user_id, price_per_mt_usd, status) VALUES
  -- real pair, pending: outstanding
  ('b1000000-0000-4000-8000-000000000001', 'd1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', NULL, 100, 'PENDING'),
  -- demo pair, pending: exempt
  ('b2000000-0000-4000-8000-000000000002', 'd4000000-0000-4000-8000-000000000004', '0dbce576-2026-5925-ab66-674d505e98ad', NULL, 100, 'PENDING'),
  -- test pair, pending: exempt
  ('b3000000-0000-4000-8000-000000000003', 'd5000000-0000-4000-8000-000000000005', '9e63f7a1-0000-4000-8000-000000000002', NULL, 100, 'PENDING'),
  -- demo buyer with a test seller, pending: outstanding
  ('b4000000-0000-4000-8000-000000000004', 'd4000000-0000-4000-8000-000000000004', '9e63f7a1-0000-4000-8000-000000000001', NULL, 100, 'PENDING'),
  -- real pair, withdrawn: counted as legacy only
  ('b5000000-0000-4000-8000-000000000005', 'd2000000-0000-4000-8000-000000000002', 'a2000000-0000-4000-8000-000000000002', NULL, 100, 'WITHDRAWN'),
  -- seller user present: counted nowhere
  ('b6000000-0000-4000-8000-000000000006', 'd6000000-0000-4000-8000-000000000006', 'a2000000-0000-4000-8000-000000000002', 'e1000000-0000-4000-8000-000000000001', 100, 'PENDING');

-- Trades: eight without full user provenance (four outstanding), one with both users.
-- These stand for rows created before snapshots and user provenance existed:
-- the snapshot version is left empty, which the table check allows for legacy
-- rows, and the snapshot guard (which refuses new trades without a snapshot)
-- is switched off only while they are inserted.
ALTER TABLE trades DISABLE TRIGGER trades_snapshot_guard;
INSERT INTO trades (id, buyer_id, seller_id, initiated_by, initiator_org_id, quantity_mt, price_per_mt_usd, status,
                    confirmed_at, delivered_at, paid_at, final_quantity_mt, final_price_per_mt, final_total_usd,
                    buyer_user_id, seller_user_id, market_snapshot_version,
                    buyer_provenance, seller_provenance) VALUES
  -- real pair, pending, no users: outstanding
  ('a3000000-0000-4000-8000-000000000001', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'PENDING_CONFIRMATION', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'REAL', 'REAL'),
  -- real pair, confirmed, only a buyer user: outstanding
  ('a3000000-0000-4000-8000-000000000002', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'CONFIRMED', now(), NULL, NULL, NULL, NULL, NULL, 'e1000000-0000-4000-8000-000000000001', NULL, NULL, 'REAL', 'REAL'),
  -- real pair, pending, both users: counted nowhere
  ('a3000000-0000-4000-8000-000000000003', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'PENDING_CONFIRMATION', NULL, NULL, NULL, NULL, NULL, NULL, 'e1000000-0000-4000-8000-000000000001', 'e2000000-0000-4000-8000-000000000002', NULL, 'REAL', 'REAL'),
  -- real pair, cancelled, no users: legacy only
  ('a3000000-0000-4000-8000-000000000004', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'CANCELLED', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'REAL', 'REAL'),
  -- demo pair, pending: exempt
  ('a3000000-0000-4000-8000-000000000005', '4da7b285-34ee-5443-9406-f96b4ed1a251', '0dbce576-2026-5925-ab66-674d505e98ad', 'BUYER', '4da7b285-34ee-5443-9406-f96b4ed1a251', 10, 100, 'PENDING_CONFIRMATION', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'DEMO', 'DEMO'),
  -- test pair, pending: exempt
  ('a3000000-0000-4000-8000-000000000006', '9e63f7a1-0000-4000-8000-000000000001', '9e63f7a1-0000-4000-8000-000000000002', 'BUYER', '9e63f7a1-0000-4000-8000-000000000001', 10, 100, 'PENDING_CONFIRMATION', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'TEST', 'TEST'),
  -- demo buyer with a test seller, pending: outstanding
  ('a3000000-0000-4000-8000-000000000007', '4da7b285-34ee-5443-9406-f96b4ed1a251', '9e63f7a1-0000-4000-8000-000000000002', 'BUYER', '4da7b285-34ee-5443-9406-f96b4ed1a251', 10, 100, 'PENDING_CONFIRMATION', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'DEMO', 'TEST'),
  -- real pair, delivered, no users: outstanding
  ('a3000000-0000-4000-8000-000000000008', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'DELIVERED', now(), now(), NULL, 10, 100, 1000, NULL, NULL, NULL, 'REAL', 'REAL'),
  -- real pair, paid, no users: legacy only
  ('a3000000-0000-4000-8000-000000000009', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'BUYER', 'a1000000-0000-4000-8000-000000000001', 10, 100, 'PAID', now(), now(), now(), 10, 100, 1000, NULL, NULL, NULL, 'REAL', 'REAL');

ALTER TABLE trades ENABLE TRIGGER trades_snapshot_guard;

-- Negotiations: six without both users (three outstanding), one with both.
-- Open negotiations of one organization pair need distinct products.
INSERT INTO negotiations (id, initiator_org_id, counterparty_org_id, initiator_user_id, counterparty_user_id,
                          product_id, quantity_mt, current_price, status, last_actor_org_id, expires_at, availability_window) VALUES
  -- real pair, open, no users: outstanding
  ('91000000-0000-4000-8000-000000000001', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', NULL, NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, 100, 'OPEN', 'a1000000-0000-4000-8000-000000000001', now() + interval '1 day', 'SPOT'),
  -- real pair, countered, only an initiator user: outstanding
  ('92000000-0000-4000-8000-000000000002', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'e1000000-0000-4000-8000-000000000001', NULL, '9510c713-6e39-5080-add3-0a7c29657b79', 10, 100, 'COUNTERED', 'a1000000-0000-4000-8000-000000000001', now() + interval '1 day', '2027-01'),
  -- real pair, expired, no users: legacy only
  ('93000000-0000-4000-8000-000000000003', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', NULL, NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, 100, 'EXPIRED', 'a1000000-0000-4000-8000-000000000001', now() - interval '1 day', '2027-02'),
  -- demo pair, open: exempt
  ('94000000-0000-4000-8000-000000000004', '4da7b285-34ee-5443-9406-f96b4ed1a251', '0dbce576-2026-5925-ab66-674d505e98ad', NULL, NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, 100, 'OPEN', '4da7b285-34ee-5443-9406-f96b4ed1a251', now() + interval '1 day', 'SPOT'),
  -- test pair, open: exempt
  ('95000000-0000-4000-8000-000000000005', '9e63f7a1-0000-4000-8000-000000000001', '9e63f7a1-0000-4000-8000-000000000002', NULL, NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, 100, 'OPEN', '9e63f7a1-0000-4000-8000-000000000001', now() + interval '1 day', 'SPOT'),
  -- demo initiator with a test counterparty, open: outstanding
  ('96000000-0000-4000-8000-000000000006', '4da7b285-34ee-5443-9406-f96b4ed1a251', '9e63f7a1-0000-4000-8000-000000000002', NULL, NULL, 'c4a688be-f7c2-5edc-8f93-6b34e387609c', 10, 100, 'OPEN', '4da7b285-34ee-5443-9406-f96b4ed1a251', now() + interval '1 day', 'SPOT'),
  -- both users present: counted nowhere
  ('97000000-0000-4000-8000-000000000007', 'a1000000-0000-4000-8000-000000000001', 'a2000000-0000-4000-8000-000000000002', 'e1000000-0000-4000-8000-000000000001', 'e2000000-0000-4000-8000-000000000002', '74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5', 10, 100, 'OPEN', 'a1000000-0000-4000-8000-000000000001', now() + interval '1 day', '2027-03');

-- Negotiation rounds: two without a proposer user, one with.
INSERT INTO negotiation_rounds (id, negotiation_id, round_number, proposer_org_id, proposer_user_id, proposed_price) VALUES
  ('98000000-0000-4000-8000-000000000001', '91000000-0000-4000-8000-000000000001', 1, 'a1000000-0000-4000-8000-000000000001', NULL, 100),
  ('98000000-0000-4000-8000-000000000002', '92000000-0000-4000-8000-000000000002', 1, 'a1000000-0000-4000-8000-000000000001', NULL, 100),
  ('98000000-0000-4000-8000-000000000003', '97000000-0000-4000-8000-000000000007', 1, 'a1000000-0000-4000-8000-000000000001', 'e1000000-0000-4000-8000-000000000001', 100);
COMMIT;
