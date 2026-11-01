-- A database with no blocker: used for the READY path of the inspection tests.
-- Load as the database owner into a freshly migrated disposable database.
BEGIN;
INSERT INTO organizations (id, name, type, verification_status, provenance, created_at) VALUES
  ('a1000000-0000-4000-8000-000000000001', 'Fixture Approved Org', 'FUEL_BUYER', 'APPROVED', 'REAL', now());
INSERT INTO users (id, email, password_hash, role, status, email_verified, organization_id,
                   kyc_status, kyc_external_evidence_reference, kyc_review_note, kyc_reviewed_at) VALUES
  -- Approved, verified, bound to an approved organization, with full KYC evidence.
  ('e1000000-0000-4000-8000-000000000001', 'alice.buyer@example.test', 'not-a-real-hash', 'BUYER', 'APPROVED', true,
     'a1000000-0000-4000-8000-000000000001', 'APPROVED', 'ref-1', 'reviewed', now()),
  -- A pending registration is reported nowhere.
  ('e5000000-0000-4000-8000-000000000005', 'erin@example.test', 'not-a-real-hash', 'BUYER', 'PENDING', false,
     NULL, 'PENDING', NULL, NULL, NULL);
UPDATE users SET kyc_reviewed_by = 'e1000000-0000-4000-8000-000000000001'
  WHERE id = 'e1000000-0000-4000-8000-000000000001';
COMMIT;
