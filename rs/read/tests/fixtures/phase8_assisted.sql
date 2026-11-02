UPDATE users SET role='ADMIN',status='APPROVED',email_verified=true,must_change_password=false,authentication_revoked_at=NULL,organization_id=NULL,kyc_status='REJECTED' WHERE id='00000000-0000-0000-0000-000000000001';
UPDATE users SET organization_id='00000000-0000-0000-0000-00000000000a',role='BUYER',status='APPROVED',email_verified=true,must_change_password=false,kyc_status='APPROVED',kyc_organization_id='00000000-0000-0000-0000-00000000000a' WHERE id='00000000-0000-0000-0000-000000000004';
UPDATE organizations SET verification_status='APPROVED',provenance='REAL';
DELETE FROM staff_capability_assignments;
DELETE FROM market_support_contexts;
INSERT INTO staff_capability_assignments(id,user_id,capability) VALUES
('00000000-0000-0000-0000-00000000003c','00000000-0000-0000-0000-000000000001','MARKET_SUPPORT_AUTHORIZATIONS'),
('00000000-0000-0000-0000-00000000003d','00000000-0000-0000-0000-000000000001','MARKET_SUPPORT_LISTINGS');
INSERT INTO market_support_contexts VALUES('00000000-0000-0000-0000-00000000003e','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-00000000000a','ASSISTED_ORDER_ENTRY','ACTIVE',now()+interval '1 hour');
