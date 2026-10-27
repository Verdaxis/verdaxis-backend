CREATE TABLE IF NOT EXISTS trades_fixture_requests(key text PRIMARY KEY,value text NOT NULL);
-- Installed only by the disposable trade test target.
CREATE OR REPLACE FUNCTION trades_fixture_seed(product uuid, point uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE buyer uuid := '00000000-0000-0000-0000-000000001000'; seller uuid := '00000000-0000-0000-0000-000000002000';
BEGIN
 TRUNCATE trades_fixture_requests;
 TRUNCATE organizations, watchlists, market_event_outbox, live_slice_benchmarks, audit_logs CASCADE;
 INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES
 (buyer,'Trade buyer','SHIPOWNER','APPROVED','REAL'),(seller,'Trade seller','FUEL_SUPPLIER','APPROVED','REAL'),
 ('00000000-0000-0000-0000-000000003000','Other buyer','SHIPOWNER','APPROVED','REAL');
 INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id) VALUES
 ('00000000-0000-0000-0000-000000001001','take-buyer@example.test','unused','BUYER','APPROVED',buyer,true,false,'APPROVED',buyer),
 ('00000000-0000-0000-0000-000000002001','take-seller@example.test','unused','SUPPLIER','APPROVED',seller,true,false,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000001002','take-member@example.test','unused','BUYER','APPROVED',buyer,true,false,'APPROVED',buyer),
 ('00000000-0000-0000-0000-000000003001','take-stranger@example.test','unused','BUYER','APPROVED','00000000-0000-0000-0000-000000003000',true,false,'APPROVED','00000000-0000-0000-0000-000000003000'),
 ('00000000-0000-0000-0000-000000004001','take-admin@example.test','unused','ADMIN','APPROVED',NULL,true,false,'REJECTED',NULL);
 INSERT INTO inventory_items(id,supplier_id,owner_user_id,fuel_type,current_stock_mt,reserved_stock_mt,incoming_stock_mt)
 VALUES('00000000-0000-0000-0000-000000005000',seller,'00000000-0000-0000-0000-000000002001','Biofuel',0,10,0);
 INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,inventory_item_id,certification_declared,msds_available,certification_scheme,specification_standard,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
 VALUES('00000000-0000-0000-0000-000000000001',seller,'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002001','SELF_SERVICE','REAL','ASK',product,point,10,10,100,'SPOT','OPEN','00000000-0000-0000-0000-000000005000',true,true,'ISCC EU','ISO 8217',10,'LIFECYCLE','residue','NL');
END $$;

CREATE OR REPLACE FUNCTION trades_fixture_state() RETURNS jsonb LANGUAGE sql AS $$
 SELECT jsonb_build_object('remaining',(SELECT remaining_quantity_mt::text FROM orderbook_orders LIMIT 1),'reserved',(SELECT reserved_stock_mt::text FROM inventory_items LIMIT 1),'current',(SELECT current_stock_mt::text FROM inventory_items LIMIT 1),'trades',(SELECT count(*) FROM trades),'audit',(SELECT count(*) FROM audit_logs),'notifications',(SELECT count(*) FROM notifications),'outbox',(SELECT count(*) FROM market_event_outbox));
$$;

CREATE OR REPLACE FUNCTION trades_fixture_change(target uuid, change text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 CASE change
 WHEN 'detached' THEN UPDATE users SET organization_id=NULL WHERE id=target;
 WHEN 'moved' THEN UPDATE users SET organization_id='00000000-0000-0000-0000-000000003000' WHERE id=target;
 WHEN 'rejected' THEN UPDATE users SET status='REJECTED' WHERE id=target;
 WHEN 'pending' THEN UPDATE users SET status='PENDING' WHERE id=target;
 WHEN 'email' THEN UPDATE users SET email_verified=false WHERE id=target;
 WHEN 'password' THEN UPDATE users SET must_change_password=true WHERE id=target;
 WHEN 'kyc' THEN UPDATE users SET kyc_status='REJECTED' WHERE id=target;
 WHEN 'supplier' THEN UPDATE users SET role='SUPPLIER' WHERE id=target;
 WHEN 'expired' THEN UPDATE orderbook_orders SET expires_at=now()-interval '1 minute' WHERE id=target;
 WHEN 'off_spec' THEN UPDATE orderbook_orders SET off_spec=true WHERE id=target;
 WHEN 'cancelled' THEN UPDATE orderbook_orders SET status='CANCELLED' WHERE id=target;
 WHEN 'confirmed' THEN UPDATE trades SET status='CONFIRMED',confirmed_at=now() WHERE id=target;
 ELSE RAISE EXCEPTION 'unknown fixture change'; END CASE;
END $$;

CREATE OR REPLACE FUNCTION trades_fixture_support(expired boolean) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE actor uuid := '00000000-0000-0000-0000-000000004001'; context uuid := '00000000-0000-0000-0000-000000006000'; BEGIN
 INSERT INTO staff_capability_assignments(id,user_id,capability,reason,granted_by_user_id) VALUES
 (gen_random_uuid(),actor,'MARKET_SUPPORT_AUTHORIZATIONS','fixture',actor),(gen_random_uuid(),actor,'MARKET_SUPPORT_LISTINGS','fixture',actor);
 INSERT INTO market_support_contexts(id,actor_user_id,organization_id,accountable_user_id,support_reference,scope,started_at,expires_at,status,version)
 VALUES(context,actor,'00000000-0000-0000-0000-000000001000',actor,'fixture','ASSISTED_ORDER_ENTRY',now()-interval '1 hour',CASE WHEN expired THEN now()-interval '1 minute' ELSE now()+interval '1 hour' END,'ACTIVE',1);
 RETURN context; END $$;

-- Named confirmation ledger fixture; the lifecycle route belongs to its component.
CREATE OR REPLACE FUNCTION trades_fixture_confirm(target uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE amount numeric;item uuid;BEGIN
 SELECT t.quantity_mt,o.inventory_item_id INTO STRICT amount,item FROM trades t JOIN orderbook_orders o ON o.id=COALESCE(t.ask_order_id,t.bid_order_id) WHERE t.id=target;
 UPDATE trades SET status='CONFIRMED',confirmed_at=now() WHERE id=target;
 UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt-amount WHERE id=item;
END $$;
CREATE OR REPLACE FUNCTION trades_fixture_audit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture audit refusal' USING ERRCODE='23514'; END $$;
CREATE OR REPLACE FUNCTION trades_fixture_reject_audit() RETURNS void LANGUAGE plpgsql AS $$ BEGIN CREATE TRIGGER trades_fixture_audit_failure BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION trades_fixture_audit_failure();END $$;
CREATE OR REPLACE FUNCTION trades_fixture_allow_audit() RETURNS void LANGUAGE plpgsql AS $$ BEGIN DROP TRIGGER IF EXISTS trades_fixture_audit_failure ON audit_logs;END $$;

CREATE OR REPLACE FUNCTION trades_fixture_assisted() RETURNS void LANGUAGE plpgsql AS $$
DECLARE auth_id uuid:=gen_random_uuid();actor uuid:='00000000-0000-0000-0000-000000004001';ord uuid:='00000000-0000-0000-0000-000000000001';context uuid;
BEGIN
 context:=trades_fixture_support(false);
 INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,order_side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,certification_declared,certification_scheme,msds_available,authorization_expires_at,status,created_by_actor_user_id,market_support_context_id,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash)
 SELECT auth_id,organization_id,actor,side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,true,'ISCC EU',true,now()+interval '1 hour','CONSUMED',actor,context,repeat('a',64),'Fixture instruction','fixture-v1','Fixture instruction',auth_id::text,repeat('b',64) FROM orderbook_orders WHERE id=ord;
 UPDATE orderbook_orders SET owner_user_id=actor,created_by_actor_user_id=actor,creation_method='MARKET_SUPPORT',support_authorization_id=auth_id WHERE id=ord;
END $$;
CREATE OR REPLACE FUNCTION trades_fixture_tie_times() RETURNS void LANGUAGE plpgsql AS $$ BEGIN UPDATE trades SET created_at='2026-10-01T00:00:00Z';END $$;

CREATE OR REPLACE FUNCTION trades_fixture_typed(product uuid,point uuid,profile text,terms jsonb) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE;BEGIN
 SELECT * INTO STRICT source FROM orderbook_orders LIMIT 1;
 DELETE FROM orderbook_orders WHERE id=source.id;
 INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,inventory_item_id,certification_declared,msds_available,certification_scheme,fame_terms,lng_terms)
 VALUES(source.id,source.organization_id,source.owner_user_id,source.created_by_actor_user_id,source.creation_method,source.provenance,source.side,product,point,source.quantity_mt,source.remaining_quantity_mt,source.price_per_mt_usd,source.availability_window,source.status,source.inventory_item_id,true,true,'ISCC EU',CASE WHEN profile='fame_v1' THEN terms ELSE NULL END,CASE WHEN profile='lng_v1' THEN terms ELSE NULL END);
END $$;

CREATE OR REPLACE FUNCTION trades_fixture_subscription(plan text,active boolean,expires timestamptz,rate numeric) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 DELETE FROM subscriptions WHERE org_id='00000000-0000-0000-0000-000000002000';
 INSERT INTO subscriptions(id,org_id,tier,is_active,expires_at,seller_fee_per_mt_usd) VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002000',plan,active,expires,rate);
END $$;
