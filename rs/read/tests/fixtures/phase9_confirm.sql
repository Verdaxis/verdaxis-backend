-- Installed only by the disposable lifecycle target after the trade fixture.
CREATE OR REPLACE FUNCTION confirm_fixture_pending(initiated text, legacy boolean DEFAULT false) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE trade_id uuid:=gen_random_uuid(); source orderbook_orders%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM orderbook_orders WHERE id='00000000-0000-0000-0000-000000000001';
 INSERT INTO trades(id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,is_anonymous,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,commission_rate_pct,commission_fee_per_mt_usd,commission_plan,fame_terms_snapshot,lng_terms_snapshot)
 SELECT trade_id,source.id,'00000000-0000-0000-0000-000000001000',source.organization_id,
 CASE WHEN legacy AND initiated='SELLER' THEN NULL ELSE '00000000-0000-0000-0000-000000001001'::uuid END,
 CASE WHEN legacy AND initiated='BUYER' THEN NULL ELSE source.owner_user_id END,
 CASE WHEN initiated='BUYER' THEN '00000000-0000-0000-0000-000000001000'::uuid ELSE source.organization_id END,
 'REAL','REAL',initiated,3,100,'PENDING_CONFIRMATION',true,source.product_id,p.name,p.fuel_type,p.fuel_grade,(SELECT value->>'code' FROM jsonb_array_elements(pg_catalog.current_setting('confirm.policy')::jsonb) value WHERE (value->>'id')::uuid=p.id),source.delivery_point_id,d.name,d.region,'SPOT',1,0,2,'free',nullif(current_setting('confirm.fame_snapshot',true),'')::jsonb,nullif(current_setting('confirm.lng_snapshot',true),'')::jsonb
 FROM products p JOIN delivery_points d ON d.id=source.delivery_point_id WHERE p.id=source.product_id;
 UPDATE orderbook_orders SET remaining_quantity_mt=7,status='PARTIALLY_FILLED' WHERE id=source.id;
 RETURN trade_id;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_referrals() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 INSERT INTO referrals(id,referrer_id,referred_user_id,referral_code_used,status,verified_at)
 SELECT gen_random_uuid(),'00000000-0000-0000-0000-000000003001',id,'lifecycle-referral','VERIFIED',now() FROM users WHERE id IN ('00000000-0000-0000-0000-000000001001','00000000-0000-0000-0000-000000002001');
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_snapshot() RETURNS jsonb LANGUAGE sql AS $$
 SELECT jsonb_build_object('trades',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t),
 'orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),
 'inventory',(SELECT jsonb_agg(to_jsonb(i) ORDER BY id) FROM inventory_items i),
 'referrals',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM referrals r),
 'audit',(SELECT count(*) FROM audit_logs),'notifications',(SELECT count(*) FROM notifications),
 'outbox',(SELECT count(*) FROM market_event_outbox),'receipts',(SELECT count(*) FROM market_command_results),
 'benchmarks',(SELECT jsonb_agg(to_jsonb(b) ORDER BY id) FROM live_slice_benchmarks b),
 'targets',(SELECT jsonb_agg(to_jsonb(w) ORDER BY id) FROM watchlist_targets w));
$$;

CREATE OR REPLACE FUNCTION confirm_fixture_withdraw(state text, reserved numeric) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 UPDATE orderbook_orders SET status=state;
 UPDATE inventory_items SET current_stock_mt=7,reserved_stock_mt=reserved;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_wait_expiry() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 UPDATE orderbook_orders SET expires_at=clock_timestamp()+interval '250 milliseconds';
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_hold_order() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 PERFORM id FROM orderbook_orders FOR UPDATE;
END $$;

CREATE OR REPLACE FUNCTION confirm_fixture_assisted() RETURNS void LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE; actor uuid:='00000000-0000-0000-0000-000000004001'; auth_id uuid:=gen_random_uuid(); context_id uuid; BEGIN
 DELETE FROM trades;
 SELECT * INTO STRICT source FROM orderbook_orders WHERE id='00000000-0000-0000-0000-000000000001';
 context_id:=trades_fixture_support(false);
 INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,order_side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,certification_declared,certification_scheme,msds_available,authorization_expires_at,status,created_by_actor_user_id,market_support_context_id,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash)
 VALUES(auth_id,source.organization_id,actor,source.side,source.product_id,source.delivery_point_id,source.availability_window,10,100,true,'ISCC EU',true,now()+interval '1 hour','CONSUMED',actor,context_id,repeat('a',64),'Fixture instruction','fixture-v1','Fixture instruction',auth_id::text,repeat('b',64));
 DELETE FROM orderbook_orders WHERE id=source.id;
 source.owner_user_id:=actor;source.created_by_actor_user_id:=actor;source.creation_method:='MARKET_SUPPORT';source.support_authorization_id:=auth_id;source.remaining_quantity_mt:=10;source.status:='OPEN';
 INSERT INTO orderbook_orders SELECT source.*;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_auto_orders() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 DELETE FROM trades;
 UPDATE products SET min_lot_size=1 WHERE id=(SELECT product_id FROM orderbook_orders LIMIT 1);
 UPDATE orderbook_orders SET remaining_quantity_mt=10,status='OPEN';
END $$;

CREATE OR REPLACE FUNCTION confirm_fixture_no_order() RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 DELETE FROM trades WHERE id=source.id;
 source.ask_order_id:=NULL;source.bid_order_id:=NULL;
 INSERT INTO trades SELECT source.*;
END $$;

CREATE OR REPLACE FUNCTION confirm_fixture_mirrors() RETURNS void LANGUAGE plpgsql AS $$
DECLARE ord orderbook_orders%ROWTYPE;product_code text;watch uuid:=gen_random_uuid();BEGIN
 SELECT * INTO STRICT ord FROM orderbook_orders LIMIT 1;
 SELECT value->>'code' INTO product_code FROM jsonb_array_elements(current_setting('confirm.policy')::jsonb) value WHERE (value->>'id')::uuid=ord.product_id;
 INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at)
 VALUES(gen_random_uuid(),'BID',product_code,ord.delivery_point_id,ord.availability_window,123,456,2,'live_slice_vwap',now(),now()),(gen_random_uuid(),'ASK',product_code,ord.delivery_point_id,ord.availability_window,100,7,1,'live_slice_vwap',now(),now());
 INSERT INTO watchlists(id,user_id,name,kind) VALUES(watch,'00000000-0000-0000-0000-000000001001','Lifecycle','CUSTOM');
 INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code,snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status)
 VALUES(gen_random_uuid(),watch,'PIN',ord.id,product_code,ord.delivery_point_id,ord.availability_window,100,10,7,'PARTIALLY_FILLED');
END $$;

CREATE OR REPLACE FUNCTION confirm_fixture_shared_inventory() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 PERFORM id FROM inventory_items FOR UPDATE;
 UPDATE inventory_items SET current_stock_mt=current_stock_mt+100;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_partial_history() RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE;BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 source.id:=gen_random_uuid();source.quantity_mt:=2;source.status:='CONFIRMED';source.confirmed_at:=now();
 INSERT INTO trades SELECT source.*;
 UPDATE orderbook_orders SET remaining_quantity_mt=5;
 UPDATE inventory_items SET reserved_stock_mt=8;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_full_take() RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE;BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 DELETE FROM trades WHERE id=source.id;
 source.quantity_mt:=10;
 INSERT INTO trades SELECT source.*;
 UPDATE orderbook_orders SET remaining_quantity_mt=0,status='FILLED';
END $$;

CREATE SEQUENCE IF NOT EXISTS confirm_fixture_attempts;
CREATE OR REPLACE FUNCTION confirm_fixture_abort_once() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=public,pg_temp AS $$ BEGIN
 IF nextval('confirm_fixture_attempts')=1 THEN RAISE EXCEPTION 'transaction attempt' USING ERRCODE='40001'; END IF;
 RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_abort_install() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 ALTER SEQUENCE confirm_fixture_attempts RESTART WITH 1;
 CREATE TRIGGER confirm_fixture_abort BEFORE UPDATE OF status ON trades FOR EACH ROW EXECUTE FUNCTION confirm_fixture_abort_once();
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_abort_remove() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 DROP TRIGGER IF EXISTS confirm_fixture_abort ON trades;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_grace() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 UPDATE users SET kyc_status='PENDING',kyc_organization_id=NULL WHERE id='00000000-0000-0000-0000-000000002001';
 INSERT INTO kyc_execution_grace(id,user_id,organization_id,kyc_status_at_enrollment,enrolled_at,expires_at,created_by) VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002000','PENDING',now(),now()+interval '1 hour','fixture');
END $$;

CREATE OR REPLACE FUNCTION confirm_fixture_null_point() RETURNS void LANGUAGE plpgsql AS $$
DECLARE t trades%ROWTYPE; o orderbook_orders%ROWTYPE; BEGIN
 SELECT * INTO STRICT t FROM trades LIMIT 1;
 SELECT * INTO STRICT o FROM orderbook_orders WHERE id=t.ask_order_id;
 DELETE FROM trades WHERE id=t.id;
 DELETE FROM watchlist_targets WHERE order_id=o.id;
 DELETE FROM orderbook_orders WHERE id=o.id;
 o.delivery_point_id:=NULL;
 INSERT INTO orderbook_orders SELECT o.*;
 t.delivery_point_id:=NULL;t.delivery_point_name:=NULL;t.delivery_point_region:=NULL;t.market_snapshot_version:=NULL;
 INSERT INTO trades SELECT t.*;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_receipt(trade_id uuid,op text,hash text,matching boolean,body jsonb) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 INSERT INTO market_command_results(id,actor_user_id,effective_organization_id,support_context_id,operation,idempotency_key,request_hash,resource_type,resource_id,response_status,response_body)
 VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002000',NULL,op,'collision',CASE WHEN matching THEN hash ELSE repeat('f',64) END,'trade',trade_id,200,body);
END $$;
