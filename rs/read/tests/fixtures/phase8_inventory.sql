-- Disposable fixture helpers run under the explicit administrator connection.
CREATE OR REPLACE FUNCTION inventory_fixture_seed(org uuid,actor uuid,port text,point uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(org,'Inventory fixture','FUEL_SUPPLIER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(actor,actor::text||'@example.test','unused','SUPPLIER','APPROVED',org,true,false,'APPROVED',org);
    INSERT INTO ports(id,name,country,location,is_active) SELECT port,name,'NL',ST_GeogFromText('POINT(4 52)'),true FROM delivery_points WHERE id=point;
END $$;
CREATE OR REPLACE FUNCTION inventory_fixture_actor(actor uuid,col text,val text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF col='role' THEN UPDATE users SET role=val WHERE id=actor;
    ELSIF col='status' THEN UPDATE users SET status=val WHERE id=actor;
    ELSIF col='email_verified' THEN UPDATE users SET email_verified=val::boolean WHERE id=actor;
    ELSIF col='must_change_password' THEN UPDATE users SET must_change_password=val::boolean WHERE id=actor;
    ELSIF col='kyc_status' THEN UPDATE users SET kyc_status=val WHERE id=actor;
    ELSE RAISE EXCEPTION 'Unclassified fixture column'; END IF;
END $$;
CREATE OR REPLACE FUNCTION inventory_fixture_reference(item uuid,org uuid,actor uuid,product uuid,point uuid,state text,trade_state text,product_code text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE ord uuid:=gen_random_uuid(); buyer uuid:=gen_random_uuid(); buyer_user uuid:=gen_random_uuid();
BEGIN
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,inventory_item_id)
        VALUES(ord,org,actor,actor,'SELF_SERVICE','REAL','ASK',product,point,10,CASE WHEN state='FILLED' THEN 0 WHEN state='PARTIALLY_FILLED' THEN 5 ELSE 10 END,100,'SPOT',state,item);
    IF trade_state IS NOT NULL THEN
        INSERT INTO organizations(id,name,type,verification_status,provenance) VALUES(buyer,'Inventory buyer fixture','SHIPPING_LINE','APPROVED','REAL');
        INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
            VALUES(buyer_user,buyer_user::text||'@example.test','unused','BUYER','APPROVED',buyer,true,false,'APPROVED',buyer);
        INSERT INTO trades(id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,buyer_provenance,seller_provenance,initiated_by,ask_order_id,is_anonymous,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version,quantity_mt,price_per_mt_usd,status,commission_rate_pct,confirmed_at,delivered_at,final_quantity_mt,final_price_per_mt,final_total_usd,created_at)
            SELECT gen_random_uuid(),buyer,org,buyer_user,actor,buyer,'REAL','REAL','BUYER',ord,false,p.id,p.name,p.fuel_type,p.fuel_grade,product_code,d.id,d.name,d.region,'SPOT',1,10,100,trade_state,0.5,CASE WHEN trade_state<>'PENDING_CONFIRMATION' THEN now() END,CASE WHEN trade_state='DELIVERED' THEN now() END,CASE WHEN trade_state='DELIVERED' THEN 10 END,CASE WHEN trade_state='DELIVERED' THEN 100 END,CASE WHEN trade_state='DELIVERED' THEN 1000 END,now() FROM products p,delivery_points d WHERE p.id=product AND d.id=point;
    END IF;
END $$;
CREATE OR REPLACE FUNCTION inventory_fixture_reject_outbox() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'Fixture append refusal'; END $$;
CREATE OR REPLACE FUNCTION inventory_fixture_outbox_failure(enabled boolean) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF enabled THEN CREATE TRIGGER inventory_fixture_outbox_refusal BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION inventory_fixture_reject_outbox();
    ELSE DROP TRIGGER IF EXISTS inventory_fixture_outbox_refusal ON market_event_outbox; END IF;
END $$;
