-- Helpers belong only to the disposable migrated publication suite.
CREATE OR REPLACE FUNCTION publish_fixture_seed(n integer, stock numeric) RETURNS void LANGUAGE plpgsql AS $$
DECLARE product uuid := 'c4cecebc-3d2d-5840-8021-57a9a11bc673'; point uuid; i integer;
BEGIN
    DROP TRIGGER IF EXISTS publish_fixture_insert_error ON orderbook_orders;
    PERFORM matching_fixture_seed('00000000-0000-0000-0000-000000000001',0,200);
    DELETE FROM orderbook_orders;
    UPDATE products SET is_active=true,min_lot_size=200 WHERE id=product;
    UPDATE delivery_points SET is_active=true;
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    INSERT INTO ports(id,name,country,location,is_active) VALUES('pub-port',' Rotterdam ','Netherlands',ST_SetSRID(ST_MakePoint(4.5,51.9),4326)::geography,true)
      ON CONFLICT(id) DO UPDATE SET name=EXCLUDED.name,is_active=true;
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,port_id,fuel_type,product_name,current_stock_mt,incoming_stock_mt,reserved_stock_mt,price_per_mt_usd,is_certified,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,off_spec,updated_at)
      SELECT '00000000-0000-0000-0000-000000004000','00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002001','pub-port','Biofuel',name,stock,0,0,100,false,true,'ISCC EU','ISO 8217:2024 RF 380',true,0,'LIFECYCLE','residue','NL',false,now() FROM products WHERE id=product;
    FOR i IN 1..n LOOP
      INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,certifications,off_spec,certification_declared,certification_scheme,msds_available)
      VALUES(('00000000-0000-0000-0000-'||lpad(to_hex(20480+i),12,'0'))::uuid,'00000000-0000-0000-0000-000000001000','00000000-0000-0000-0000-000000001001','00000000-0000-0000-0000-000000001001','SELF_SERVICE','REAL','BID',product,point,200,200,110,'SPOT','OPEN','[]',false,false,NULL,false);
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_expired(n integer, same_item boolean) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source inventory_items%ROWTYPE; point uuid; item uuid; i integer;
BEGIN
    SELECT * INTO STRICT source FROM inventory_items WHERE id='00000000-0000-0000-0000-000000004000';
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    item := CASE WHEN same_item THEN source.id ELSE '00000000-0000-0000-0000-000000004001'::uuid END;
    IF NOT same_item THEN
      INSERT INTO inventory_items(id,supplier_id,owner_user_id,port_id,fuel_type,product_name,current_stock_mt,reserved_stock_mt,incoming_stock_mt,updated_at)
      VALUES(item,source.supplier_id,source.owner_user_id,source.port_id,source.fuel_type,source.product_name,0,0,0,now());
    END IF;
    UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt+n*200 WHERE id=item;
    FOR i IN 1..n LOOP
      INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,inventory_item_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,expires_at,version,off_spec,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
      SELECT ('00000000-0000-0000-0000-'||lpad(to_hex(24576+i),12,'0'))::uuid,source.supplier_id,source.owner_user_id,source.owner_user_id,'SELF_SERVICE','REAL','ASK',p.id,point,item,200,200,100,'SPOT','OPEN',now()-interval '1 second',1,false,true,'ISCC EU','ISO 8217:2024 RF 380',true,0,'LIFECYCLE','residue','NL' FROM products p WHERE p.name=source.product_name;
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_change(change text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    CASE change
    WHEN 'filled' THEN UPDATE orderbook_orders SET status='FILLED',remaining_quantity_mt=0; UPDATE inventory_items SET reserved_stock_mt=0;
    WHEN 'incompatible' THEN UPDATE orderbook_orders SET certification_scheme='REDcert-EU',certifications='["REDcert-EU"]';
    WHEN 'expired' THEN UPDATE orderbook_orders SET expires_at=now()-interval '1 second';
    END CASE;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_expiry(at_time timestamptz) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET expires_at=at_time WHERE side::text='ASK';
$$;

CREATE OR REPLACE FUNCTION publish_fixture_order_error() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    RAISE EXCEPTION 'publication insertion unavailable' USING ERRCODE='23514';
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_key_row(k text, h text) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE listing uuid := gen_random_uuid();
BEGIN
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,version,idempotency_key,idempotency_operation,idempotency_request_hash)
    SELECT listing,i.supplier_id,i.owner_user_id,i.owner_user_id,'SELF_SERVICE',o.provenance,'ASK',p.id,d.id,200,0,100,'SPOT','FILLED',1,k,'inventory.publish',h FROM inventory_items i JOIN organizations o ON o.id=i.supplier_id JOIN products p ON p.name=i.product_name CROSS JOIN delivery_points d WHERE i.id='00000000-0000-0000-0000-000000004000' AND d.name='Rotterdam';
    RETURN listing;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_provenance(kind text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE target uuid := CASE WHEN kind='DEMO' THEN 'd1e43e55-3fb0-4b5e-9f0b-93aa10050222'::uuid ELSE '00000000-0000-0000-0000-000000009000'::uuid END;
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(target,'Publication provenance fixture','FUEL_SUPPLIER','APPROVED',kind);
    UPDATE users SET organization_id=target,kyc_organization_id=target WHERE id='00000000-0000-0000-0000-000000002001';
    UPDATE inventory_items SET supplier_id=target;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_fame(declaration jsonb) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    UPDATE ports SET name='Singapore' WHERE id='pub-port';
    UPDATE inventory_items SET product_name=(SELECT name FROM products WHERE id='e561e43f-d9b2-598e-981c-f1d28d515ddc'),fuel_type='FAME',fame_terms=declaration;
END $$;

CREATE OR REPLACE FUNCTION publish_fixture_refuse_effect() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
    IF NEW.action='inventory.published' THEN RAISE EXCEPTION 'publication effect unavailable' USING ERRCODE='23514'; END IF;
    RETURN NEW;
END $$;
