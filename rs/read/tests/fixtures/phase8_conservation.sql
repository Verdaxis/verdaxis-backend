-- Disposable conservation fixture helpers.
CREATE OR REPLACE FUNCTION conservation_reset() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM effect_fixture_reset();
END $$;

CREATE OR REPLACE FUNCTION conservation_seed(supplier_org uuid, supplier uuid, buyer_org uuid, buyer uuid, port text, point uuid, product uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,type,verification_status,provenance)
    VALUES(supplier_org,'Conservation supplier','FUEL_SUPPLIER','APPROVED','REAL'),
          (buyer_org,'Conservation buyer','SHIPOWNER','APPROVED','REAL');
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
    VALUES(supplier,supplier::text||'@example.test','unused','SUPPLIER','APPROVED',supplier_org,true,false,'APPROVED',supplier_org),
          (buyer,buyer::text||'@example.test','unused','BUYER','APPROVED',buyer_org,true,false,'APPROVED',buyer_org);
    -- This disposable catalog uses a one-tonne minimum for its finite sequences.
    UPDATE products SET min_lot_size=1 WHERE id=product;
    INSERT INTO ports(id,name,country,location,is_active)
    SELECT port,name,'NL',ST_GeogFromText('POINT(4 52)'),true FROM delivery_points WHERE id=point;
END $$;

CREATE OR REPLACE FUNCTION conservation_step(operation text, item uuid, ord uuid, trade uuid, amount numeric, product uuid, point uuid, buyer_org uuid, buyer uuid, code text) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE stock inventory_items%ROWTYPE; book orderbook_orders%ROWTYPE; bid orderbook_orders%ROWTYPE; hit trades%ROWTYPE; new_id uuid; new_remaining numeric; filled numeric;
BEGIN
    IF operation='fixture:lock' THEN
        PERFORM pg_advisory_xact_lock(amount::bigint);
        PERFORM id FROM orderbook_orders WHERE id=ord FOR UPDATE;
        RETURN '{}'::jsonb;
    END IF;
    SELECT * INTO STRICT stock FROM inventory_items WHERE id=item FOR UPDATE;
    IF NOT EXISTS (SELECT 1 FROM users u JOIN organizations o ON o.id=u.organization_id WHERE u.id=stock.owner_user_id AND u.organization_id=stock.supplier_id AND u.role='SUPPLIER' AND u.status='APPROVED' AND u.email_verified AND NOT u.must_change_password AND u.kyc_status='APPROVED' AND u.kyc_organization_id=o.id AND o.verification_status='APPROVED' AND o.provenance='REAL') OR NOT EXISTS (SELECT 1 FROM users u JOIN organizations o ON o.id=u.organization_id WHERE u.id=buyer AND o.id=buyer_org AND u.role='BUYER' AND u.status='APPROVED' AND u.email_verified AND NOT u.must_change_password AND u.kyc_status='APPROVED' AND u.kyc_organization_id=o.id AND o.verification_status='APPROVED' AND o.provenance='REAL') THEN
        RAISE EXCEPTION 'fixture admission';
    END IF;
    IF buyer_org=stock.supplier_id OR NOT EXISTS (SELECT 1 FROM products p WHERE p.id=product AND p.is_active AND p.min_lot_size=1 AND p.unit='MT') OR NOT EXISTS (SELECT 1 FROM delivery_points d JOIN ports p ON p.id=stock.port_id AND p.name=d.name WHERE d.id=point AND d.is_active AND p.is_active) OR stock.price_per_mt_usd<>100 THEN
        RAISE EXCEPTION 'fixture catalog domain';
    END IF;
    IF EXISTS (SELECT 1 FROM orderbook_orders o WHERE o.inventory_item_id=item AND (o.organization_id<>stock.supplier_id OR o.owner_user_id IS DISTINCT FROM stock.owner_user_id OR o.created_by_actor_user_id IS DISTINCT FROM stock.owner_user_id OR o.creation_method<>'SELF_SERVICE' OR o.provenance<>'REAL' OR o.side<>'ASK' OR o.product_id<>product OR o.delivery_point_id<>point OR o.availability_window<>'SPOT' OR o.price_per_mt_usd<>100 OR o.off_spec OR NOT o.certification_declared OR o.certification_scheme<>'ISCC EU' OR NOT o.msds_available OR o.fame_terms IS NOT NULL OR o.lng_terms IS NOT NULL)) THEN
        RAISE EXCEPTION 'fixture order domain';
    END IF;
    IF operation='fixture:publish' THEN
        SELECT * INTO book FROM orderbook_orders WHERE inventory_item_id=item AND status IN ('OPEN','PARTIALLY_FILLED') AND (expires_at IS NULL OR expires_at>now()) LIMIT 1;
        IF FOUND THEN RETURN jsonb_build_object('order',book.id); END IF;
        IF EXISTS (SELECT 1 FROM orderbook_orders WHERE inventory_item_id=item AND status IN ('OPEN','PARTIALLY_FILLED') AND expires_at<=now()) OR stock.current_stock_mt<=0 THEN RETURN '{"refused":true}'::jsonb; END IF;
        new_id:=gen_random_uuid();
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,inventory_item_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        VALUES(new_id,stock.supplier_id,stock.owner_user_id,stock.owner_user_id,'SELF_SERVICE','REAL','ASK',product,point,item,stock.current_stock_mt,stock.current_stock_mt,100,'SPOT','OPEN',false,true,'ISCC EU',true,10,'LIFECYCLE','residue','NL');
        UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt+current_stock_mt,current_stock_mt=0 WHERE id=item;
        RETURN jsonb_build_object('order',new_id);
    END IF;
    IF operation='fixture:bid' THEN
        IF amount<1 OR amount>100000 OR scale(amount)>2 THEN RETURN '{"refused":true}'::jsonb; END IF;
        new_id:=gen_random_uuid();
        INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
        VALUES(new_id,buyer_org,buyer,buyer,'SELF_SERVICE','REAL','BID',product,point,amount,amount,100,'SPOT','OPEN',false,true,'ISCC EU',true,10,'LIFECYCLE','residue','NL');
        RETURN jsonb_build_object('order',new_id);
    END IF;
    IF ord IS NULL THEN RETURN '{"refused":true}'::jsonb; END IF;
    SELECT * INTO book FROM orderbook_orders WHERE id=ord AND inventory_item_id=item FOR UPDATE;
    IF NOT FOUND THEN RETURN '{"refused":true}'::jsonb; END IF;
    IF operation='fixture:elapse' THEN
        IF book.status NOT IN ('OPEN','PARTIALLY_FILLED') THEN RETURN '{"refused":true}'::jsonb; END IF;
        UPDATE orderbook_orders SET expires_at=now()-interval '1 second' WHERE id=ord;
        RETURN '{}'::jsonb;
    ELSIF operation='fixture:fill' THEN
        SELECT * INTO bid FROM orderbook_orders WHERE id=trade AND side='BID' AND organization_id=buyer_org AND inventory_item_id IS NULL AND status IN ('OPEN','PARTIALLY_FILLED') FOR UPDATE;
        IF NOT FOUND OR book.status NOT IN ('OPEN','PARTIALLY_FILLED') OR (book.expires_at IS NOT NULL AND book.expires_at<=now()) THEN RETURN '{"refused":true}'::jsonb; END IF;
        IF bid.owner_user_id IS DISTINCT FROM buyer OR bid.created_by_actor_user_id IS DISTINCT FROM buyer OR bid.provenance<>'REAL' OR bid.product_id<>product OR bid.delivery_point_id<>point OR bid.availability_window<>'SPOT' OR bid.price_per_mt_usd<book.price_per_mt_usd OR bid.fame_terms IS NOT NULL OR bid.lng_terms IS NOT NULL OR bid.off_spec OR (bid.expires_at IS NOT NULL AND bid.expires_at<=now()) THEN RAISE EXCEPTION 'fixture matching domain'; END IF;
        filled:=least(book.remaining_quantity_mt,bid.remaining_quantity_mt);
        IF stock.reserved_stock_mt<filled THEN RAISE EXCEPTION 'fixture fill shortage'; END IF;
        new_id:=gen_random_uuid();
        INSERT INTO trades(id,ask_order_id,bid_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,initiated_by,quantity_mt,price_per_mt_usd,status,confirmed_at,buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version)
        SELECT new_id,ord,bid.id,buyer_org,stock.supplier_id,buyer,stock.owner_user_id,buyer_org,'BUYER',filled,book.price_per_mt_usd,'CONFIRMED',now(),'REAL','REAL',p.id,p.name,p.fuel_type,p.fuel_grade,code,d.id,d.name,d.region,'SPOT',1 FROM products p JOIN delivery_points d ON d.id=point WHERE p.id=product;
        UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt-filled WHERE id=item;
        UPDATE orderbook_orders SET remaining_quantity_mt=remaining_quantity_mt-filled,status=CASE WHEN remaining_quantity_mt=filled THEN 'FILLED' ELSE 'PARTIALLY_FILLED' END WHERE id IN (ord,bid.id);
        RETURN jsonb_build_object('trade',new_id);
    ELSIF operation='fixture:take' THEN
        IF book.status NOT IN ('OPEN','PARTIALLY_FILLED') OR (book.expires_at IS NOT NULL AND book.expires_at<=now()) OR amount<=0 OR amount>book.remaining_quantity_mt THEN RETURN '{"refused":true}'::jsonb; END IF;
        new_id:=gen_random_uuid();
        INSERT INTO trades(id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,initiated_by,quantity_mt,price_per_mt_usd,status,buyer_provenance,seller_provenance,product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,availability_window,market_snapshot_version)
        SELECT new_id,ord,buyer_org,stock.supplier_id,buyer,stock.owner_user_id,buyer_org,'BUYER',amount,100,'PENDING_CONFIRMATION','REAL','REAL',p.id,p.name,p.fuel_type,p.fuel_grade,code,d.id,d.name,d.region,'SPOT',1 FROM products p JOIN delivery_points d ON d.id=point WHERE p.id=product;
        new_remaining:=book.remaining_quantity_mt-amount;
        UPDATE orderbook_orders SET remaining_quantity_mt=new_remaining,status=CASE WHEN new_remaining=0 THEN 'FILLED' ELSE 'PARTIALLY_FILLED' END WHERE id=ord;
        RETURN jsonb_build_object('trade',new_id);
    ELSIF operation='fixture:amend' THEN
        IF book.status NOT IN ('OPEN','PARTIALLY_FILLED') OR (book.expires_at IS NOT NULL AND book.expires_at<=now()) THEN RETURN '{"refused":true}'::jsonb; END IF;
        filled:=book.quantity_mt-book.remaining_quantity_mt;
        IF amount<filled OR amount<1 THEN RETURN '{"refused":true}'::jsonb; END IF;
        new_remaining:=amount-filled;
        IF new_remaining-book.remaining_quantity_mt>stock.current_stock_mt THEN RETURN '{"refused":true}'::jsonb; END IF;
        UPDATE inventory_items SET current_stock_mt=current_stock_mt-(new_remaining-book.remaining_quantity_mt),reserved_stock_mt=reserved_stock_mt+(new_remaining-book.remaining_quantity_mt) WHERE id=item;
        UPDATE orderbook_orders SET quantity_mt=amount,remaining_quantity_mt=new_remaining,status=CASE WHEN new_remaining=0 THEN 'FILLED' WHEN new_remaining=amount THEN 'OPEN' ELSE 'PARTIALLY_FILLED' END,version=version+1 WHERE id=ord;
        RETURN '{}'::jsonb;
    ELSIF operation IN ('fixture:confirm','fixture:decline') THEN
        SELECT * INTO hit FROM trades WHERE id=trade AND ask_order_id=ord AND status='PENDING_CONFIRMATION' FOR UPDATE;
        IF NOT FOUND THEN RETURN '{"refused":true}'::jsonb; END IF;
        IF hit.buyer_id<>buyer_org OR hit.seller_id<>stock.supplier_id OR hit.buyer_user_id IS DISTINCT FROM buyer OR hit.seller_user_id IS DISTINCT FROM stock.owner_user_id OR hit.initiator_org_id<>buyer_org OR hit.initiated_by<>'BUYER' OR hit.product_id<>product OR hit.delivery_point_id<>point OR hit.availability_window<>'SPOT' OR hit.buyer_provenance<>'REAL' OR hit.seller_provenance<>'REAL' THEN RAISE EXCEPTION 'fixture counterparty domain'; END IF;
        IF stock.reserved_stock_mt<hit.quantity_mt THEN RETURN '{"refused":true}'::jsonb; END IF;
        IF operation='fixture:confirm' THEN
            UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt-hit.quantity_mt WHERE id=item;
            UPDATE trades SET status='CONFIRMED',confirmed_at=now() WHERE id=trade;
        ELSIF book.status IN ('CANCELLED','EXPIRED') THEN
            UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt-hit.quantity_mt,current_stock_mt=current_stock_mt+hit.quantity_mt WHERE id=item;
            UPDATE trades SET status='DECLINED' WHERE id=trade;
        ELSE
            new_remaining:=book.remaining_quantity_mt+hit.quantity_mt;
            UPDATE trades SET status='DECLINED' WHERE id=trade;
            IF book.expires_at IS NOT NULL AND book.expires_at<=now() THEN
                IF stock.reserved_stock_mt<new_remaining THEN RAISE EXCEPTION 'fixture release shortage'; END IF;
                UPDATE inventory_items SET reserved_stock_mt=reserved_stock_mt-new_remaining,current_stock_mt=current_stock_mt+new_remaining WHERE id=item;
                UPDATE orderbook_orders SET remaining_quantity_mt=new_remaining,status='EXPIRED',version=version+1 WHERE id=ord;
            ELSE
                UPDATE orderbook_orders SET remaining_quantity_mt=new_remaining,status=CASE WHEN new_remaining=quantity_mt THEN 'OPEN' ELSE 'PARTIALLY_FILLED' END,version=version+1 WHERE id=ord;
            END IF;
        END IF;
        RETURN '{}'::jsonb;
    END IF;
    RAISE EXCEPTION 'unknown fixture transition %',operation;
END $$;

CREATE OR REPLACE FUNCTION conservation_hold(kind text,item uuid,key bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF kind='slice' THEN PERFORM pg_advisory_xact_lock(key);
    ELSIF kind='inventory' THEN PERFORM id FROM inventory_items WHERE id=item FOR UPDATE;
    ELSE RAISE EXCEPTION 'unknown fixture hold'; END IF;
END $$;

CREATE OR REPLACE FUNCTION conservation_race_take(item uuid,ord uuid,product uuid,point uuid,buyer_org uuid,buyer uuid,code text) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE key bigint;
BEGIN
    key:=('x'||substr(encode(sha256(convert_to(product::text||'|'||point::text||'|SPOT','UTF8')),'hex'),1,16))::bit(64)::bigint;
    PERFORM pg_advisory_xact_lock(CASE WHEN key=0 THEN 1 ELSE key END);
    PERFORM id FROM orderbook_orders WHERE id=ord FOR UPDATE;
    RETURN conservation_step('fixture:take',item,ord,NULL,4,product,point,buyer_org,buyer,code);
END $$;
