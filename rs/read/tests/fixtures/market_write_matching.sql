-- Helpers are installed only in the migrated disposable test database.
CREATE OR REPLACE FUNCTION matching_fixture_seed(aggressor uuid, n integer, qty numeric)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE
    buyer uuid := '00000000-0000-0000-0000-000000001000';
    seller uuid := '00000000-0000-0000-0000-000000002000';
    buyer_user uuid := '00000000-0000-0000-0000-000000001001';
    seller_user uuid := '00000000-0000-0000-0000-000000002001';
    product uuid := 'c4cecebc-3d2d-5840-8021-57a9a11bc673';
    point uuid;
    candidate uuid;
    i integer;
BEGIN
    TRUNCATE organizations, watchlists, market_event_outbox, live_slice_benchmarks, audit_logs CASCADE;
    DROP TRIGGER IF EXISTS matching_fixture_trade_check ON trades;
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    INSERT INTO organizations(id,name,type,verification_status,provenance)
        VALUES(buyer,'Matching buyer','SHIPOWNER','APPROVED','REAL'),
              (seller,'Matching seller','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(buyer_user,'matching-buyer@example.test','unused','BUYER','APPROVED',buyer,true,false,'APPROVED',buyer),
              (seller_user,'matching-seller@example.test','unused','SUPPLIER','APPROVED',seller,true,false,'APPROVED',seller);
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
    VALUES(aggressor,buyer,buyer_user,buyer_user,'SELF_SERVICE','REAL','BID',product,point,qty,qty,110,'SPOT','OPEN',
        false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL');
    FOR i IN 1..n LOOP
        candidate := ('00000000-0000-0000-0000-' || lpad(to_hex(i+1),12,'0'))::uuid;
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
            product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
            off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
            carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,created_at)
        VALUES(candidate,seller,seller_user,seller_user,'SELF_SERVICE','REAL','ASK',product,point,1,1,100,'SPOT','OPEN',
            false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL',
            '2026-10-24T12:00:00Z'::timestamptz - i*interval '1 second');
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_unavailable(expired uuid, cancelled uuid, at_time timestamptz)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE orderbook_orders SET expires_at=at_time WHERE id=expired;
    UPDATE orderbook_orders SET status='CANCELLED' WHERE id=cancelled;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_priority(first_id uuid, second_id uuid, third_id uuid)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE ordinal bigint;
BEGIN
    SELECT nextval('orderbook_acceptance_ordinal_seq') INTO ordinal;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+3 WHERE id=first_id;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+2 WHERE id=second_id;
    UPDATE orderbook_orders SET acceptance_ordinal=ordinal+1 WHERE id=third_id;
    PERFORM setval('orderbook_acceptance_ordinal_seq',ordinal+3);
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:00Z'::timestamptz WHERE id=first_id;
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:01Z'::timestamptz WHERE id=second_id;
    UPDATE orderbook_orders SET created_at='2026-10-24T12:00:02Z'::timestamptz WHERE id=third_id;
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_reject_trade(resting uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    EXECUTE 'CREATE OR REPLACE FUNCTION matching_fixture_trade_check() RETURNS trigger LANGUAGE plpgsql AS $body$ BEGIN IF NEW.ask_order_id=' || quote_literal(resting) || '::uuid THEN RAISE EXCEPTION ''fixture trade refusal'' USING ERRCODE=''23514''; END IF; RETURN NEW; END $body$';
    CREATE TRIGGER matching_fixture_trade_check BEFORE INSERT ON trades FOR EACH ROW EXECUTE FUNCTION matching_fixture_trade_check();
END $$;

CREATE OR REPLACE FUNCTION matching_fixture_back_inventory(ord uuid,item uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE;
BEGIN
    SELECT * INTO STRICT source FROM orderbook_orders WHERE id=ord;
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,fuel_type,current_stock_mt,reserved_stock_mt,incoming_stock_mt,updated_at)
        SELECT item,organization_id,owner_user_id,'Biofuel',0,remaining_quantity_mt,0,'2026-10-24T12:00:00Z' FROM orderbook_orders WHERE id=ord;
    DELETE FROM orderbook_orders WHERE id=ord;
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,inventory_item_id)
    VALUES(source.id,source.organization_id,source.owner_user_id,source.created_by_actor_user_id,source.creation_method,source.provenance,source.side,
        source.product_id,source.delivery_point_id,source.quantity_mt,source.remaining_quantity_mt,source.price_per_mt_usd,source.availability_window,source.status,
        source.off_spec,source.certification_declared,source.certification_scheme,source.specification_standard,source.msds_available,
        source.carbon_intensity_gco2_mj,source.carbon_intensity_method,source.feedstock,source.origin,item);
END $$;
CREATE OR REPLACE FUNCTION matching_fixture_second_aggressor(source_id uuid,new_id uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        SELECT new_id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin FROM orderbook_orders WHERE id=source_id;
END $$;
