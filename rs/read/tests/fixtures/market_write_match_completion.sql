-- Helpers for match completion tests on a migrated disposable database.
CREATE OR REPLACE FUNCTION completion_fixture_referral() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO referrals(id, referrer_id, referred_user_id, referral_code_used, status)
    VALUES ('00000000-0000-0000-0000-000000008001',
            '00000000-0000-0000-0000-000000001001',
            '00000000-0000-0000-0000-000000002001', 'VDX-FIXTURE', 'VERIFIED');
END $$;

CREATE OR REPLACE FUNCTION completion_fixture_subscription(plan text, active boolean, expiry timestamptz, negotiated numeric)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    DELETE FROM subscriptions WHERE org_id='00000000-0000-0000-0000-000000002000';
    INSERT INTO subscriptions(id,org_id,tier,is_active,expires_at,seller_fee_per_mt_usd,created_at,updated_at)
    VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002000',plan,active,expiry,negotiated,now(),now());
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_short_stock(item uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE inventory_items SET reserved_stock_mt=0 WHERE id=item;
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_watch(ord uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE watch uuid:=gen_random_uuid();
BEGIN
    INSERT INTO watchlists(id,user_id,name,kind) VALUES(watch,'00000000-0000-0000-0000-000000001001','Completion','CUSTOM');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code,
        snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status)
    SELECT gen_random_uuid(),watch,'PIN',o.id,'B30',o.delivery_point_id,o.availability_window,100,1,1,'OPEN'
    FROM orderbook_orders o JOIN products p ON p.id=o.product_id WHERE o.id=ord;
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,availability_window_code)
    SELECT gen_random_uuid(),watch,'SLICE',market_product_code,delivery_point_id,availability_window_code FROM watchlist_targets WHERE order_id=ord;
END $$;
