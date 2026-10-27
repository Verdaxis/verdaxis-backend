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
    INSERT INTO subscriptions(id,org_id,tier,is_active,expires_at,seller_fee_per_mt_usd)
    VALUES(gen_random_uuid(),'00000000-0000-0000-0000-000000002000',plan,active,expiry,negotiated);
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

CREATE OR REPLACE FUNCTION completion_fixture_back_inventory(ord uuid,item uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE ordinal bigint;
BEGIN
    SELECT acceptance_ordinal INTO STRICT ordinal FROM orderbook_orders WHERE id=ord;
    PERFORM matching_fixture_back_inventory(ord,item);
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal WHERE id=ord;
END $$;

CREATE OR REPLACE FUNCTION completion_fixture_terms(profile text,bid_terms jsonb,ask_terms jsonb) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid; point uuid; source orderbook_orders%ROWTYPE;
BEGIN
    product:=CASE WHEN profile='fame' THEN 'e561e43f-d9b2-598e-981c-f1d28d515ddc'::uuid ELSE '8f6f34a5-2927-54a6-8564-cca40ae7cb8a'::uuid END;
    SELECT id INTO STRICT point FROM delivery_points WHERE name=CASE WHEN profile='fame' THEN 'Singapore' ELSE 'Rotterdam' END;
    FOR source IN SELECT * FROM orderbook_orders ORDER BY id LOOP
        DELETE FROM orderbook_orders WHERE id=source.id;
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
            product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
            off_spec,certification_declared,certification_scheme,msds_available,fame_terms,lng_terms,acceptance_ordinal)
        VALUES(source.id,source.organization_id,source.owner_user_id,source.created_by_actor_user_id,source.creation_method,source.provenance,source.side,
            product,point,source.quantity_mt,source.remaining_quantity_mt,source.price_per_mt_usd,source.availability_window,source.status,
            false,true,'ISCC EU',true,
            CASE WHEN profile='fame' THEN CASE WHEN source.side='BID' THEN bid_terms ELSE ask_terms END ELSE NULL END,
            CASE WHEN profile='lng' THEN CASE WHEN source.side='BID' THEN bid_terms ELSE ask_terms END ELSE NULL END,source.acceptance_ordinal);
    END LOOP;
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_amend_source() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET price_per_mt_usd=price_per_mt_usd+1,version=version+1;
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_extra_member() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    PERFORM matching_fixture_member('00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002002','SUPPLIER');
END $$;

CREATE OR REPLACE FUNCTION completion_fixture_link(ord uuid,item uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE;
BEGIN
    SELECT * INTO STRICT source FROM orderbook_orders WHERE id=ord;
    DELETE FROM orderbook_orders WHERE id=ord;
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,inventory_item_id,acceptance_ordinal)
    VALUES(source.id,source.organization_id,source.owner_user_id,source.created_by_actor_user_id,source.creation_method,source.provenance,source.side,
        source.product_id,source.delivery_point_id,source.quantity_mt,source.remaining_quantity_mt,source.price_per_mt_usd,source.availability_window,source.status,
        source.off_spec,source.certification_declared,source.certification_scheme,source.specification_standard,source.msds_available,
        source.carbon_intensity_gco2_mj,source.carbon_intensity_method,source.feedstock,source.origin,item,source.acceptance_ordinal);
    UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt+source.remaining_quantity_mt WHERE id=item;
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_price(ord uuid,price numeric) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET price_per_mt_usd=price WHERE id=ord;
END $$;

-- Statement triggers count even a rebuild whose DELETE finds no row.
CREATE TABLE IF NOT EXISTS completion_benchmark_calls(operation text NOT NULL);
CREATE TABLE IF NOT EXISTS completion_benchmark_deleted(side text NOT NULL);
CREATE OR REPLACE FUNCTION completion_benchmark_call() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=public,pg_temp AS $$ BEGIN
    INSERT INTO completion_benchmark_calls VALUES(TG_OP);
    RETURN NULL;
END $$;
CREATE OR REPLACE FUNCTION completion_benchmark_delete() RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path=public,pg_temp AS $$ BEGIN
    INSERT INTO completion_benchmark_deleted VALUES(OLD.side);
    RETURN OLD;
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_benchmark_probe() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    DROP TRIGGER IF EXISTS completion_benchmark_statement ON live_slice_benchmarks;
    DROP TRIGGER IF EXISTS completion_benchmark_row ON live_slice_benchmarks;
    TRUNCATE completion_benchmark_calls,completion_benchmark_deleted;
    INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,order_count,source,created_at,updated_at)
    SELECT gen_random_uuid(),s.side,'BIOFUEL_B30',o.delivery_point_id,o.availability_window,999,999,99,'live_slice_vwap',now(),now()
    FROM (SELECT * FROM orderbook_orders LIMIT 1) o CROSS JOIN (VALUES('BID'),('ASK')) s(side);
    CREATE TRIGGER completion_benchmark_statement AFTER INSERT OR DELETE ON live_slice_benchmarks FOR EACH STATEMENT EXECUTE FUNCTION completion_benchmark_call();
    CREATE TRIGGER completion_benchmark_row AFTER DELETE ON live_slice_benchmarks FOR EACH ROW EXECUTE FUNCTION completion_benchmark_delete();
END $$;
CREATE OR REPLACE FUNCTION completion_fixture_benchmark_probe_remove() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    DROP TRIGGER IF EXISTS completion_benchmark_statement ON live_slice_benchmarks;
    DROP TRIGGER IF EXISTS completion_benchmark_row ON live_slice_benchmarks;
END $$;
