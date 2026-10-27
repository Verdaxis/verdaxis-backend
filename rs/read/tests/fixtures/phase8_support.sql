-- Disposable support fixtures preserve the application table identities.
CREATE OR REPLACE FUNCTION support_fixture_seed(actor uuid,target uuid,org uuid,other uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    TRUNCATE organizations,market_event_outbox,audit_logs CASCADE;
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES
        (org,'Support customer','FUEL_SUPPLIER','APPROVED','REAL'),
        (other,'Pending customer','FUEL_SUPPLIER','PENDING','UNKNOWN');
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password,kyc_status)
        VALUES(actor,actor::text||'@example.test','unused','ADMIN','APPROVED',true,false,'PENDING'),
        (target,target::text||'@example.test','unused','ADMIN','APPROVED',true,false,'PENDING');
END $$;
CREATE OR REPLACE FUNCTION support_fixture_context(context_id uuid,actor uuid,org uuid,state text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO market_support_contexts(id,actor_user_id,organization_id,accountable_user_id,support_reference,scope,started_at,expires_at,status,version)
        VALUES(context_id,actor,org,actor,'Support request','ASSISTED_ORDER_ENTRY',now(),now()+interval '1 hour',state,1);
END $$;
CREATE OR REPLACE FUNCTION support_fixture_audit_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.action='market_support.context_revoked' THEN
        RAISE EXCEPTION 'fixture audit refusal';
    END IF;
    RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION support_fixture_audit_failure(enabled boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS support_fixture_audit_guard ON audit_logs;
    IF enabled THEN
        CREATE TRIGGER support_fixture_audit_guard BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION support_fixture_audit_guard();
    END IF;
END $$;

CREATE OR REPLACE FUNCTION support_fixture_listing(actor uuid,org uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid; point uuid; auth uuid; ord uuid; i integer;
BEGIN
    SELECT p.id,dp.id INTO product,point FROM products p CROSS JOIN delivery_points dp
        WHERE p.name='Bio Methanol' AND dp.name='Singapore' LIMIT 1;
    FOR i IN 1..2 LOOP
        auth:=gen_random_uuid(); ord:=gen_random_uuid();
        INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,status,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,certification_declared,msds_available,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id)
            VALUES(auth,org,actor,'ACTIVE',product,point,'SPOT',10,100,'ASK',now()+interval '1 day',now()+interval '2 days',true,true,repeat('a',64),'Customer instruction','v1','Consent reference',auth::text,repeat('b',64),actor);
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,support_authorization_id,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,msds_available,version)
            VALUES(ord,org,actor,actor,'MARKET_SUPPORT',auth,'REAL','ASK',product,point,10,10,100,'SPOT','OPEN',false,true,'ISCC EU',true,1);
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION support_fixture_user(account uuid,account_role text,account_status text,change_required boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password,kyc_status)
        VALUES(account,account::text||'@example.test','unused',account_role,account_status,true,change_required,'PENDING');
END $$;
CREATE OR REPLACE FUNCTION support_fixture_organization(org uuid,label text,review_state text,origin text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(org,label,'FUEL_SUPPLIER',review_state,origin);
END $$;
CREATE OR REPLACE FUNCTION support_fixture_lng_authorization(actor uuid,org uuid,terms text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid; point uuid; auth uuid := gen_random_uuid();
BEGIN
    SELECT p.id,dp.id INTO product,point FROM products p CROSS JOIN delivery_points dp
        WHERE p.name='Bio-LNG' AND dp.name='Rotterdam' LIMIT 1;
    INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,status,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,order_side,authorization_expires_at,order_expires_at,certification_declared,msds_available,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash,created_by_actor_user_id,lng_terms)
        VALUES(auth,org,actor,'ACTIVE',product,point,'SPOT',10,100,'ASK',now()+interval '1 day',now()+interval '2 days',true,true,repeat('a',64),'Customer instruction','v1','Consent reference',auth::text,repeat('b',64),actor,terms::json);
END $$;
