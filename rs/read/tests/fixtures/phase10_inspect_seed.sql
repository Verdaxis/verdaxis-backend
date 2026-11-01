-- Synthetic rows for the Phase 10 inspection tests, loaded as the database
-- owner into a freshly migrated disposable database. Every identifier is a
-- fixed literal so the expected counts in the tests can be read from this file.
--
-- Expected counts (the Python report prints the same names):
--   ineligible_users = 3                      duplicate_case_insensitive_emails = 1
--   legacy_orders_without_owner = 5           outstanding_legacy_orders_without_owner = 2
--   legacy_rfqs_without_owner = 5             outstanding_legacy_rfqs_without_owner = 2
--   legacy_quotes_without_owner = 5           outstanding_legacy_quotes_without_owner = 2
--   legacy_trades_without_user_provenance = 8 outstanding_legacy_trades_without_user_provenance = 4
--   legacy_negotiations_without_user_provenance = 6
--   outstanding_legacy_negotiations_without_user_provenance = 3
--   legacy_negotiation_rounds_without_user_provenance = 2
--   approved_kyc_without_external_review_evidence = 2
--   pending_organization_join_reviews = 2
BEGIN;

-- Organizations. A1 is approved; A2 is pending. D1/D2 are canonical demo
-- organizations and T1/T2 are known test organizations.
INSERT INTO organizations (id, name, type, verification_status, provenance, created_at) VALUES
  ('a1000000-0000-4000-8000-000000000001', 'Fixture Approved Org', 'FUEL_BUYER', 'APPROVED', 'REAL', now()),
  ('a2000000-0000-4000-8000-000000000002', 'Fixture Pending Org', 'FUEL_SUPPLIER', 'PENDING', 'REAL', now()),
  ('4da7b285-34ee-5443-9406-f96b4ed1a251', 'Verdaxis Demo Buyer 01', 'FUEL_BUYER', 'APPROVED', 'DEMO', now()),
  ('0dbce576-2026-5925-ab66-674d505e98ad', 'Verdaxis Demo Buyer 02', 'FUEL_BUYER', 'APPROVED', 'DEMO', now()),
  ('9e63f7a1-0000-4000-8000-000000000001', 'Fixture Test Org 1', 'FUEL_BUYER', 'APPROVED', 'TEST', now()),
  ('9e63f7a1-0000-4000-8000-000000000002', 'Fixture Test Org 2', 'FUEL_SUPPLIER', 'APPROVED', 'TEST', now());

-- Users. The first three emails exercise masking; no value here is a real
-- address.
INSERT INTO users (id, email, password_hash, role, status, email_verified, organization_id,
                   kyc_status, kyc_external_evidence_reference, kyc_review_note, kyc_reviewed_by, kyc_reviewed_at) VALUES
  -- Eligible: approved, verified, bound to an approved organization, full KYC evidence.
  ('e1000000-0000-4000-8000-000000000001', 'alice.buyer@example.test', 'not-a-real-hash', 'BUYER', 'APPROVED', true,
     'a1000000-0000-4000-8000-000000000001', 'APPROVED', 'ref-1', 'reviewed', NULL, now()),
  -- Ineligible: email not verified.
  ('e2000000-0000-4000-8000-000000000002', 'bob.supplier@example.test', 'not-a-real-hash', 'SUPPLIER', 'APPROVED', false,
     'a1000000-0000-4000-8000-000000000001', 'PENDING', NULL, NULL, NULL, NULL),
  -- Ineligible: no organization.
  ('e3000000-0000-4000-8000-000000000003', 'carol@example.test', 'not-a-real-hash', 'BUYER', 'APPROVED', true,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  -- Ineligible: organization not approved.
  ('e4000000-0000-4000-8000-000000000004', 'dave@example.test', 'not-a-real-hash', 'BUYER', 'APPROVED', true,
     'a2000000-0000-4000-8000-000000000002', 'PENDING', NULL, NULL, NULL, NULL),
  -- Not counted: pending, unverified, unbound.
  ('e5000000-0000-4000-8000-000000000005', 'erin@example.test', 'not-a-real-hash', 'BUYER', 'PENDING', false,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  -- Not counted: administrators are not buyers or suppliers.
  ('e6000000-0000-4000-8000-000000000006', 'frank.admin@example.test', 'not-a-real-hash', 'ADMIN', 'APPROVED', false,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  -- Not counted: no role.
  ('e7000000-0000-4000-8000-000000000007', 'grace@example.test', 'not-a-real-hash', NULL, 'APPROVED', false,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  -- Duplicate pair after lower(trim(email)); the unique index only folds case.
  ('e8000000-0000-4000-8000-000000000008', 'dup@example.test', 'not-a-real-hash', 'ADMIN', 'REJECTED', false,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  ('e9000000-0000-4000-8000-000000000009', ' dup@example.test', 'not-a-real-hash', 'ADMIN', 'REJECTED', false,
     NULL, 'PENDING', NULL, NULL, NULL, NULL),
  -- KYC approved with a missing review note, and with no evidence at all.
  ('ea000000-0000-4000-8000-00000000000a', 'heidi@example.test', 'not-a-real-hash', 'ADMIN', 'REJECTED', false,
     NULL, 'APPROVED', 'ref-2', NULL, NULL, now()),
  ('eb000000-0000-4000-8000-00000000000b', 'ivan@example.test', 'not-a-real-hash', 'ADMIN', 'REJECTED', false,
     NULL, 'APPROVED', NULL, NULL, NULL, NULL),
  -- Not counted: KYC not approved, so missing evidence is expected.
  ('ec000000-0000-4000-8000-00000000000c', 'judy@example.test', 'not-a-real-hash', 'ADMIN', 'REJECTED', false,
     NULL, 'REJECTED', NULL, NULL, NULL, NULL);
-- The eligible user reviews themselves; the reviewer column has a foreign key
-- so it is set after the row exists.
UPDATE users SET kyc_reviewed_by = 'e1000000-0000-4000-8000-000000000001'
  WHERE id = 'e1000000-0000-4000-8000-000000000001';

-- Organization join requests: two pending, one approved, one rejected.
INSERT INTO organization_join_requests (id, user_id, organization_id, status) VALUES
  ('f1000000-0000-4000-8000-000000000001', 'e3000000-0000-4000-8000-000000000003', 'a1000000-0000-4000-8000-000000000001', 'PENDING'),
  ('f2000000-0000-4000-8000-000000000002', 'e4000000-0000-4000-8000-000000000004', 'a1000000-0000-4000-8000-000000000001', 'PENDING'),
  ('f3000000-0000-4000-8000-000000000003', 'e5000000-0000-4000-8000-000000000005', 'a1000000-0000-4000-8000-000000000001', 'APPROVED'),
  ('f4000000-0000-4000-8000-000000000004', 'e6000000-0000-4000-8000-000000000006', 'a1000000-0000-4000-8000-000000000001', 'REJECTED');
COMMIT;
