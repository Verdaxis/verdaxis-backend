-- Installed only by the disposable lifecycle target after the trade fixture.
CREATE OR REPLACE FUNCTION confirm_fixture_pending(initiated text, legacy boolean DEFAULT false) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE trade_id uuid:=gen_random_uuid(); source orderbook_orders%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM orderbook_orders WHERE id='00000000-0000-0000-0000-000000000001';
 IF initiated='SELLER' THEN
  UPDATE orderbook_orders SET inventory_item_id=NULL WHERE id=source.id;
 END IF;
 INSERT INTO trades(id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,is_anonymous,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,commission_rate_pct,commission_fee_per_mt_usd,commission_plan)
 SELECT trade_id,source.id,'00000000-0000-0000-0000-000000001000',source.organization_id,
 CASE WHEN legacy AND initiated='SELLER' THEN NULL ELSE '00000000-0000-0000-0000-000000001001'::uuid END,
 CASE WHEN legacy AND initiated='BUYER' THEN NULL ELSE source.owner_user_id END,
 CASE WHEN initiated='BUYER' THEN '00000000-0000-0000-0000-000000001000'::uuid ELSE source.organization_id END,
 'REAL','REAL',initiated,3,100,'PENDING_CONFIRMATION',true,source.product_id,p.name,p.fuel_type,p.fuel_grade,p.code,source.delivery_point_id,d.name,d.region,'SPOT',1,0,2,'FREE'
 FROM products p JOIN delivery_points d ON d.id=source.delivery_point_id WHERE p.id=source.product_id;
 UPDATE orderbook_orders SET remaining_quantity_mt=7,status='PARTIALLY_FILLED' WHERE id=source.id;
 RETURN trade_id;
END $$;
CREATE OR REPLACE FUNCTION confirm_fixture_referrals() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 INSERT INTO referrals(id,referrer_user_id,referred_user_id,referral_code,status,verified_at)
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
