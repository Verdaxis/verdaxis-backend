-- Fixture helpers run only in the explicitly migrated disposable database.
CREATE OR REPLACE FUNCTION effect_fixture_reset() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    TRUNCATE organizations, watchlists, market_event_outbox,
        live_slice_benchmarks, audit_logs CASCADE;
END $$;

CREATE OR REPLACE FUNCTION effect_fixture_seed(
    org uuid, actor uuid, ord uuid, opposite uuid, watch uuid, target uuid,
    product uuid, point uuid, product_code text, standard text
) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(org,'Effect fixture','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,
        email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(actor,actor::text || '@example.test','unused','SUPPLIER','APPROVED',
            org,true,false,'APPROVED',org);
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,
        creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,
        remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,
        certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        VALUES(ord,org,actor,actor,'SELF_SERVICE','REAL','ASK',product,point,10,10,
            100,'SPOT','OPEN',false,true,'ISCC EU',standard,true,10,'LIFECYCLE','residue','NL'),
            (opposite,org,actor,actor,'SELF_SERVICE','REAL','BID',product,point,10,10,
            90,'SPOT','OPEN',false,true,'ISCC EU',standard,true,10,'LIFECYCLE','residue','NL');
    INSERT INTO watchlists(id,user_id,name,kind) VALUES(watch,actor,'Effects','CUSTOM');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,order_id,market_product_code,
        delivery_point_id,availability_window_code,snapshot_price_per_mt_usd,
        snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status)
        VALUES(target,watch,'PIN',ord,product_code,point,'SPOT',100,10,10,'OPEN');
END $$;

CREATE OR REPLACE FUNCTION effect_fixture_stale_benchmarks() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO live_slice_benchmarks(id,side,market_product,delivery_point_id,
        availability_window,benchmark_price_per_mt_usd,total_remaining_quantity_mt,
        order_count,source)
        SELECT gen_random_uuid(),side,wt.market_product_code,o.delivery_point_id,
            'SPOT',1,1,1,'live_slice_vwap'
        FROM orderbook_orders o CROSS JOIN watchlist_targets wt;
END $$;

CREATE OR REPLACE FUNCTION effect_fixture_expiry(ord uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE orderbook_orders SET expires_at=now()-interval '1 second' WHERE id=ord;
END $$;

CREATE OR REPLACE FUNCTION effect_fixture_mixed_slice(ord uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE demo_org uuid := '79609f48-0a3e-560e-a1e1-63d90601d84a';
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(demo_org,'Verdaxis Demo Supplier 01','FUEL_SUPPLIER','APPROVED','DEMO');
    INSERT INTO orderbook_orders(id,organization_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,
        availability_window,status,expires_at,off_spec,certification_declared,
        certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        SELECT gen_random_uuid(),demo_org,'SYSTEM','DEMO','ASK',product_id,
            delivery_point_id,20,20,80,availability_window,'OPEN',now()+interval '1 day',
            false,true,certification_scheme,specification_standard,true,
            carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin
        FROM orderbook_orders WHERE id=ord;
END $$;
