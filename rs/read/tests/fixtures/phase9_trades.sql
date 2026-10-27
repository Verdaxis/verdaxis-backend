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
 INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,inventory_item_id,certification_declared,msds_available,certification_scheme)
 VALUES('00000000-0000-0000-0000-000000000001',seller,'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002001','SELF_SERVICE','REAL','ASK',product,point,10,10,100,'SPOT','OPEN','00000000-0000-0000-0000-000000005000',true,true,'ISCC EU');
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
