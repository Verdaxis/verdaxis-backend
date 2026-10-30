-- Installed only in fresh disposable databases by the concurrent component suite.
CREATE OR REPLACE FUNCTION races_seed() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS create_fixture_abort ON orderbook_orders;
    PERFORM publish_fixture_seed(0,1000);
    UPDATE products SET min_lot_size=1 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673';
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password)
    VALUES('00000000-0000-0000-0000-000000003001','race-admin@example.test','unused','ADMIN','APPROVED',true,false);
END $$;

CREATE OR REPLACE FUNCTION races_expire(target uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET expires_at=now()-interval '1 minute' WHERE id=target;
$$;
CREATE OR REPLACE FUNCTION races_second_item() RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE target uuid:=gen_random_uuid();
BEGIN
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,port_id,fuel_type,product_name,current_stock_mt,incoming_stock_mt,reserved_stock_mt,price_per_mt_usd,is_certified,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec)
    SELECT target,supplier_id,owner_user_id,port_id,fuel_type,product_name,200,0,0,price_per_mt_usd,is_certified,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec FROM inventory_items WHERE id='00000000-0000-0000-0000-000000004000';
    RETURN target;
END $$;

CREATE OR REPLACE FUNCTION races_pin(target uuid, code text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE watch uuid:=gen_random_uuid();
BEGIN
    INSERT INTO watchlists(id,user_id,name,kind) VALUES(watch,'00000000-0000-0000-0000-000000001001','Concurrent market','CUSTOM');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,delivery_point_id,availability_window_code,snapshot_price_per_mt_usd,snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status)
    SELECT gen_random_uuid(),watch,'PIN',id,code,delivery_point_id,availability_window,price_per_mt_usd,quantity_mt,remaining_quantity_mt,status::text FROM orderbook_orders WHERE id=target;
END $$;

CREATE OR REPLACE FUNCTION races_hold() RETURNS void LANGUAGE sql AS $$ SELECT pg_advisory_lock(290030001) $$;
CREATE OR REPLACE FUNCTION races_release() RETURNS boolean LANGUAGE sql AS $$ SELECT pg_advisory_unlock(290030001) $$;
CREATE OR REPLACE FUNCTION races_observe_release() RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE observed integer; deadline timestamptz:=clock_timestamp()+interval '8 seconds'; seen_at timestamptz;
BEGIN
    LOOP
        PERFORM pg_stat_clear_snapshot();
        SELECT a.pid INTO observed FROM pg_stat_activity a WHERE a.datname=current_database()
          AND a.application_name='races-second' AND a.wait_event_type='Lock'
          AND EXISTS(SELECT 1 FROM pg_stat_activity b WHERE b.pid=ANY(pg_blocking_pids(a.pid)) AND b.application_name='races-first') LIMIT 1;
        IF observed IS NOT NULL THEN
            seen_at:=clock_timestamp();
            PERFORM races_release();
            RETURN jsonb_build_object('pid',observed,'release_ms',extract(epoch FROM clock_timestamp()-seen_at)*1000);
        END IF;
        IF clock_timestamp()>deadline THEN
            PERFORM races_release();
            RAISE EXCEPTION 'second route did not wait on first route';
        END IF;
        PERFORM pg_sleep(0.001);
    END LOOP;
END $$;
CREATE OR REPLACE FUNCTION races_effect_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('application_name')='races-first' THEN
        -- The observation barrier is separate from the economic row-lock budget.
        PERFORM set_config('lock_timeout','10s',true);
        PERFORM pg_advisory_xact_lock(290030001);
        PERFORM set_config('lock_timeout','500ms',true);
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS races_audit_barrier ON audit_logs;
DROP TRIGGER IF EXISTS races_outbox_barrier ON market_event_outbox;
CREATE TRIGGER races_audit_barrier BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION races_effect_barrier();
CREATE TRIGGER races_outbox_barrier BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION races_effect_barrier();

CREATE OR REPLACE FUNCTION races_invariants() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'stock',NOT EXISTS(SELECT 1 FROM inventory_items i WHERE current_stock_mt<0 OR reserved_stock_mt<0 OR incoming_stock_mt<0 OR scale(current_stock_mt)>2 OR scale(reserved_stock_mt)>2 OR scale(incoming_stock_mt)>2 OR reserved_stock_mt<>(
   COALESCE((SELECT sum(o.remaining_quantity_mt) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status::text IN ('OPEN','PARTIALLY_FILLED')),0)+
   COALESCE((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=COALESCE(t.ask_order_id,t.bid_order_id) WHERE o.inventory_item_id=i.id AND t.status::text='PENDING_CONFIRMATION'),0))),
 'orders',NOT EXISTS(SELECT 1 FROM orderbook_orders o WHERE o.remaining_quantity_mt<0 OR o.remaining_quantity_mt>o.quantity_mt OR (o.status::text IN ('OPEN','PARTIALLY_FILLED') AND o.remaining_quantity_mt<>o.quantity_mt-COALESCE((SELECT sum(t.quantity_mt) FROM trades t WHERE (t.ask_order_id=o.id OR t.bid_order_id=o.id) AND t.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')),0))),
 'money',NOT EXISTS(SELECT 1 FROM trades WHERE final_quantity_mt IS NOT NULL AND (final_quantity_mt<=0 OR final_quantity_mt>quantity_mt OR final_total_usd<>round(final_quantity_mt*final_price_per_mt,2))),
 'freeze',NOT EXISTS(SELECT 1 FROM trade_freezes GROUP BY trade_id HAVING count(*) FILTER(WHERE status='ACTIVE')>1));
$$;

CREATE OR REPLACE FUNCTION races_publication_product(product uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE products SET min_lot_size=1,is_active=true WHERE id=product;
    UPDATE inventory_items SET product_name=(SELECT name FROM products WHERE id=product),specification_standard=NULL WHERE id='00000000-0000-0000-0000-000000004000';
END $$;
