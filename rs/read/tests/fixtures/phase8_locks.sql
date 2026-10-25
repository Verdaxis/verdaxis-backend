CREATE TABLE organizations(id uuid PRIMARY KEY, verification_status text NOT NULL DEFAULT 'APPROVED');
CREATE TABLE users(id uuid PRIMARY KEY, organization_id uuid, role text NOT NULL, status text NOT NULL DEFAULT 'APPROVED', email_verified boolean NOT NULL DEFAULT true, must_change_password boolean NOT NULL DEFAULT false, password_changed_at timestamptz, authentication_revoked_at timestamptz, kyc_status text NOT NULL DEFAULT 'PENDING', kyc_organization_id uuid);
CREATE TABLE kyc_execution_grace(id uuid PRIMARY KEY, user_id uuid, organization_id uuid, expires_at timestamptz);
CREATE TABLE trades(id uuid PRIMARY KEY, buyer_id uuid NOT NULL,seller_id uuid NOT NULL,buyer_user_id uuid,seller_user_id uuid,bid_order_id uuid,ask_order_id uuid);
CREATE TABLE orderbook_orders(id uuid PRIMARY KEY, organization_id uuid,owner_user_id uuid,created_by_actor_user_id uuid,creation_method text,side text);
CREATE TABLE inventory_items(id uuid PRIMARY KEY);
INSERT INTO organizations(id) VALUES('00000000-0000-0000-0000-00000000000a'),('00000000-0000-0000-0000-000000000014');
INSERT INTO users(id,organization_id,role,status,email_verified,must_change_password,authentication_revoked_at) VALUES
('00000000-0000-0000-0000-000000000001',NULL,'BUYER','REJECTED',false,true,now()+interval '1 minute'),
('00000000-0000-0000-0000-000000000002','00000000-0000-0000-0000-000000000014','SUPPLIER','APPROVED',true,false,NULL),
('00000000-0000-0000-0000-000000000003',NULL,'BUYER','APPROVED',true,false,NULL),
('00000000-0000-0000-0000-000000000004','00000000-0000-0000-0000-00000000000a','BUYER','APPROVED',true,false,NULL);
INSERT INTO trades VALUES('00000000-0000-0000-0000-00000000001e','00000000-0000-0000-0000-00000000000a','00000000-0000-0000-0000-000000000014','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-000000000002',NULL,NULL);
INSERT INTO kyc_execution_grace VALUES('00000000-0000-0000-0000-000000000032','00000000-0000-0000-0000-000000000001','00000000-0000-0000-0000-00000000000a',now()-interval '1 hour');

ALTER TABLE organizations ADD COLUMN provenance text NOT NULL DEFAULT 'REAL';
CREATE TABLE lock_effects(id integer PRIMARY KEY,value integer NOT NULL DEFAULT 0);
INSERT INTO lock_effects(id) VALUES(1);
INSERT INTO orderbook_orders(id) VALUES('00000000-0000-0000-0000-00000000005a');
CREATE TABLE staff_capability_assignments(id uuid PRIMARY KEY,user_id uuid,capability text,revoked_at timestamptz,expires_at timestamptz);
CREATE TABLE market_support_contexts(id uuid PRIMARY KEY,actor_user_id uuid,organization_id uuid,scope text,status text,expires_at timestamptz);
