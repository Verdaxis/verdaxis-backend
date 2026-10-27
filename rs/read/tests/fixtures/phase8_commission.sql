-- Helpers are installed only in the migrated disposable database.
CREATE OR REPLACE FUNCTION commission_fixture_reset(a uuid,b uuid,c uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    TRUNCATE organizations,commissions,audit_logs,notifications,market_event_outbox CASCADE;
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password)
    VALUES(a,a::text||'@example.test','unused','ADMIN','APPROVED',true,false),
          (b,b::text||'@example.test','unused','ADMIN','APPROVED',true,false),
          (c,c::text||'@example.test','unused','BUYER','APPROVED',true,false);
END $$;
CREATE OR REPLACE FUNCTION commission_fixture_rows(n integer) RETURNS void LANGUAGE plpgsql AS $$
DECLARE buyer uuid:=gen_random_uuid(); seller uuid:=gen_random_uuid(); listing uuid:=gen_random_uuid(); parent uuid; i integer;
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(buyer,'Legacy buyer','SHIPOWNER','APPROVED','REAL'),(seller,'Legacy supplier','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO public_listings(id,supplier_id,region,fuel_type,fuel_grade,quantity_mt,price_per_mt_usd,availability_window,is_verdaxis_verified,status,created_at,updated_at)
    VALUES(listing,seller,'Europe','Methanol','GREEN',10,100,'SPOT',false,'ACTIVE',now(),now());
    FOR i IN 1..n LOOP
        parent:=gen_random_uuid();
        INSERT INTO orders(id,listing_id,buyer_id,status,buyer_accepted_terms_at,commission_rate_pct,created_at) VALUES(parent,listing,buyer,'PENDING',now(),1,now());
        INSERT INTO commissions(id,match_id,amount_usd,status,created_at,updated_at) VALUES(gen_random_uuid(),parent,123.45,'PENDING','2026-10-25T00:00:00Z','2026-10-25T00:00:00Z');
    END LOOP;
END $$;
CREATE OR REPLACE FUNCTION commission_fixture_amounts() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE commissions SET amount_usd=9999999999.99,status=CASE WHEN right(id::text,1) IN ('0','1','2','3','4') THEN 'PENDING' WHEN right(id::text,1) IN ('5','6','7','8','9') THEN 'INVOICED' ELSE 'PAID' END;
END $$;
CREATE OR REPLACE FUNCTION commission_fixture_audit_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'audit append unavailable'; END $$;
CREATE OR REPLACE FUNCTION commission_fixture_refuse_audit(enabled boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS commission_fixture_audit_guard ON audit_logs;
    IF enabled THEN CREATE TRIGGER commission_fixture_audit_guard BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION commission_fixture_audit_guard(); END IF;
END $$;
CREATE OR REPLACE FUNCTION commission_fixture_trade(product uuid,point uuid,code text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE buyer uuid:=gen_random_uuid(); seller uuid:=gen_random_uuid();
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(buyer,'Commission buyer','SHIPOWNER','APPROVED','REAL'),(seller,'Commission seller','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO trades(id,buyer_id,seller_id,initiator_org_id,initiated_by,quantity_mt,price_per_mt_usd,commission_rate_pct,commission_fee_per_mt_usd,commission_plan,commission_amount_usd,status,created_at,buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version)
    SELECT gen_random_uuid(),buyer,seller,buyer,'BUYER',10,100,0,2,'free',20,'PENDING_CONFIRMATION',now(),'REAL','REAL',p.id,p.name,p.fuel_type,p.fuel_grade,code,d.id,d.name,d.region,'SPOT',1 FROM products p CROSS JOIN delivery_points d WHERE p.id=product AND d.id=point;
END $$;

CREATE OR REPLACE FUNCTION commission_fixture_account(actor uuid, account_status text, password_change boolean)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE users SET status=account_status, must_change_password=password_change WHERE id=actor;
END $$;
CREATE OR REPLACE FUNCTION commission_fixture_mark_after_lock()
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    LOCK TABLE commissions IN ACCESS EXCLUSIVE MODE;
    UPDATE commissions SET invoice_number='AFTER-COMMIT';
END $$;
