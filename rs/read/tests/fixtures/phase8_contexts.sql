-- Disposable context fixtures use the application tables and constraints.
CREATE OR REPLACE FUNCTION context_fixture_context(context_id uuid,actor uuid,org uuid,stale boolean,state text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO market_support_contexts(id,actor_user_id,organization_id,accountable_user_id,support_reference,scope,started_at,expires_at,status,version)
    VALUES(context_id,actor,org,actor,'Context case','ASSISTED_ORDER_ENTRY',now()-interval '2 hours',CASE WHEN stale THEN now()-interval '1 hour' ELSE now()+interval '1 hour' END,state,1);
END $$;

CREATE OR REPLACE FUNCTION context_fixture_capability(actor uuid,kind text,state text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF state='missing' THEN
        DELETE FROM staff_capability_assignments WHERE user_id=actor AND capability=kind;
    ELSE
        INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id,granted_at,expires_at,revoked_at,revoked_by_user_id,revocation_reason)
        VALUES(gen_random_uuid(),actor,kind,'Context case',actor,now()-interval '1 day',CASE WHEN state='expired' THEN now()-interval '1 hour' ELSE now()+interval '1 day' END,CASE WHEN state='revoked' THEN now() ELSE NULL END,CASE WHEN state='revoked' THEN actor ELSE NULL END,CASE WHEN state='revoked' THEN 'Context case' ELSE NULL END)
        ON CONFLICT(user_id,capability) DO UPDATE SET expires_at=EXCLUDED.expires_at,revoked_at=EXCLUDED.revoked_at,revoked_by_user_id=EXCLUDED.revoked_by_user_id,revocation_reason=EXCLUDED.revocation_reason;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION context_fixture_member(org uuid,review_org uuid,label text,member_role text,member_status text,verified boolean,forced boolean,kyc text,first text,last text,grace_seconds integer) RETURNS void LANGUAGE plpgsql AS $$
DECLARE member uuid := gen_random_uuid();
BEGIN
    INSERT INTO users(id,organization_id,email,password_hash,role,status,email_verified,must_change_password,kyc_status,kyc_organization_id,first_name,last_name)
    VALUES(member,org,label||'@example.test','unused',member_role,member_status,verified,forced,kyc,review_org,first,last);
    IF grace_seconds<>0 THEN
        INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES(gen_random_uuid(),member,org,kyc,now()-interval '1 day',now()+make_interval(secs=>grace_seconds),'fixture');
    END IF;
END $$;
CREATE OR REPLACE FUNCTION context_fixture_members(org uuid,other uuid,n integer) RETURNS void LANGUAGE plpgsql AS $$
DECLARE i integer;
BEGIN
    PERFORM context_fixture_member(org,org,'approved','SUPPLIER','APPROVED',true,false,'APPROVED','First','Last',0);
    PERFORM context_fixture_member(org,org,'empty','SUPPLIER','APPROVED',true,false,'APPROVED',' ',NULL,0);
    PERFORM context_fixture_member(org,NULL,'grace','SUPPLIER','APPROVED',true,false,'PENDING',NULL,NULL,3600);
    PERFORM context_fixture_member(org,NULL,'expired','SUPPLIER','APPROVED',true,false,'PENDING',NULL,NULL,-3600);
    PERFORM context_fixture_member(org,NULL,'pendingkyc','SUPPLIER','APPROVED',true,false,'PENDING',NULL,NULL,0);
    PERFORM context_fixture_member(org,org,'buyer','BUYER','APPROVED',true,false,'APPROVED',NULL,NULL,0);
    PERFORM context_fixture_member(org,org,'pending','SUPPLIER','PENDING',true,false,'APPROVED',NULL,NULL,0);
    PERFORM context_fixture_member(org,org,'unverified','SUPPLIER','APPROVED',false,false,'APPROVED',NULL,NULL,0);
    PERFORM context_fixture_member(org,org,'forced','SUPPLIER','APPROVED',true,true,'APPROVED',NULL,NULL,0);
    PERFORM context_fixture_member(org,org,'rejected','SUPPLIER','APPROVED',true,false,'REJECTED',NULL,NULL,3600);
    PERFORM context_fixture_member(org,other,'foreign','SUPPLIER','APPROVED',true,false,'APPROVED',NULL,NULL,0);
    FOR i IN 1..n LOOP
        PERFORM context_fixture_member(org,org,'supplier-'||lpad(i::text,4,'0'),'SUPPLIER','APPROVED',true,false,'APPROVED',NULL,NULL,0);
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION context_fixture_lists(actor uuid,org uuid,n integer) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid; point uuid; auth uuid; ord uuid; i integer;
BEGIN
    SELECT p.id,dp.id INTO product,point FROM products p CROSS JOIN delivery_points dp WHERE p.name='Bio Methanol' AND dp.name='Singapore' LIMIT 1;
    FOR i IN 1..n LOOP
        auth:=gen_random_uuid(); ord:=gen_random_uuid();
        INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,status,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,certification_declared,msds_available,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,created_at)
        VALUES(auth,org,actor,'ACTIVE',product,point,'SPOT',10,100,'ASK',now()+interval '1 day',now()+interval '2 days',true,true,repeat('a',64),'Customer instruction','v1','Consent reference',auth::text,repeat('b',64),actor,now()-make_interval(secs=>i));
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,support_authorization_id,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,msds_available,version,created_at)
        VALUES(ord,org,actor,actor,'MARKET_SUPPORT',auth,'REAL','ASK',product,point,10,10,100,'SPOT','OPEN',false,true,'ISCC EU',true,1,now()-make_interval(secs=>i));
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION context_fixture_fame_authorization(actor uuid,org uuid,terms text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid; point uuid; auth uuid := gen_random_uuid();
BEGIN
    SELECT p.id,dp.id INTO product,point FROM products p CROSS JOIN delivery_points dp WHERE p.name='UCOME B100' AND dp.name='Singapore' LIMIT 1;
    INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,status,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,certification_declared,msds_available,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,fame_terms)
    VALUES(auth,org,actor,'ACTIVE',product,point,'SPOT',10,100,'ASK',now()+interval '1 day',now()+interval '2 days',true,true,repeat('a',64),'Customer instruction','v1','Consent reference',auth::text,repeat('b',64),actor,terms::json);
END $$;

CREATE OR REPLACE FUNCTION context_fixture_audit_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.action='market_support.context_started' THEN RAISE EXCEPTION 'fixture audit refusal'; END IF;
    RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION context_fixture_projection_guard() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER AS $$
BEGIN
    -- The response identity read must still succeed before commit.
    REVOKE SELECT ON organizations FROM rust_app;
    RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION context_fixture_failure(kind text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS context_fixture_audit_guard ON audit_logs;
    DROP TRIGGER IF EXISTS context_fixture_projection_guard ON market_support_contexts;
    IF kind='audit' THEN
        CREATE TRIGGER context_fixture_audit_guard BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION context_fixture_audit_guard();
    ELSIF kind='projection' THEN
        CREATE TRIGGER context_fixture_projection_guard AFTER INSERT ON market_support_contexts FOR EACH ROW EXECUTE FUNCTION context_fixture_projection_guard();
    END IF;
END $$;
