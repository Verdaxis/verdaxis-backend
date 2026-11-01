-- Installed only in the migrated disposable database of the offboarding
-- market target. Every row uses the B30 product at Rotterdam, SPOT.

CREATE OR REPLACE FUNCTION offboarding_market_member(organization uuid, organization_type text, member uuid, member_role text)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    IF organization IS NOT NULL THEN
        INSERT INTO organizations(id,name,type,verification_status,provenance)
            VALUES(organization,'Offboarding ' || organization,organization_type,'APPROVED','REAL')
            ON CONFLICT (id) DO NOTHING;
    END IF;
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id)
        VALUES(member,'offboarding-' || member || '@example.test','unused',member_role,'APPROVED',organization,true,false,
            CASE WHEN member_role='ADMIN' THEN 'REJECTED' ELSE 'APPROVED' END,organization);
END $$;

CREATE OR REPLACE FUNCTION offboarding_market_inventory(item uuid, owner uuid, current_stock numeric, reserved_stock numeric)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO inventory_items(id,supplier_id,owner_user_id,fuel_type,current_stock_mt,reserved_stock_mt,incoming_stock_mt)
        SELECT item,organization_id,owner,'Biofuel',current_stock,reserved_stock,0 FROM users WHERE id=owner;
$$;

-- The owner's current organization owns the order. A BID crosses every ASK.
CREATE OR REPLACE FUNCTION offboarding_market_order(order_id uuid, owner uuid, order_side text, quantity numeric, remaining numeric, order_status text, item uuid)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE point uuid;
BEGIN
    SELECT id INTO STRICT point FROM delivery_points WHERE name='Rotterdam';
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,
        product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,inventory_item_id,
        off_spec,certification_declared,certification_scheme,specification_standard,msds_available,
        carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin)
    SELECT order_id,organization_id,owner,owner,'SELF_SERVICE','REAL',order_side,
        'c4cecebc-3d2d-5840-8021-57a9a11bc673',point,quantity,remaining,CASE WHEN order_side='BID' THEN 110 ELSE 100 END,'SPOT',order_status,item,
        false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL'
    FROM users WHERE id=owner;
END $$;

-- The recorded people's current organizations are the trade sides. Settled
-- states carry the delivery and payment fields the lifecycle CHECK requires.
CREATE OR REPLACE FUNCTION offboarding_market_trade(trade_id uuid, bid uuid, ask uuid, buyer uuid, seller uuid, quantity numeric, trade_status text)
RETURNS void LANGUAGE sql AS $$
    INSERT INTO trades(id,bid_order_id,ask_order_id,buyer_id,seller_id,buyer_user_id,seller_user_id,initiator_org_id,
        buyer_provenance,seller_provenance,initiated_by,quantity_mt,price_per_mt_usd,status,confirmed_at,
        delivered_at,paid_at,final_quantity_mt,final_price_per_mt,final_total_usd,is_anonymous,
        product_id,product_name,fuel_type,fuel_grade,market_product,delivery_point_id,delivery_point_name,delivery_point_region,
        availability_window,market_snapshot_version,commission_rate_pct,commission_fee_per_mt_usd,commission_plan)
    SELECT trade_id,bid,ask,b.organization_id,s.organization_id,buyer,seller,b.organization_id,
        'REAL','REAL','BUYER',quantity,100,trade_status,
        CASE WHEN trade_status<>'PENDING_CONFIRMATION' THEN '2026-10-20T00:00:00Z'::timestamptz END,
        CASE WHEN trade_status IN ('DELIVERED','PAID') THEN '2026-10-21T00:00:00Z'::timestamptz END,
        CASE WHEN trade_status='PAID' THEN '2026-10-22T00:00:00Z'::timestamptz END,
        CASE WHEN trade_status IN ('DELIVERED','PAID') THEN quantity END,
        CASE WHEN trade_status IN ('DELIVERED','PAID') THEN 100 END,
        CASE WHEN trade_status IN ('DELIVERED','PAID') THEN quantity*100 END,true,
        p.id,p.name,p.fuel_type,p.fuel_grade,'B30',d.id,d.name,d.region,'SPOT',1,0,2,'free'
    FROM users b, users s, products p, delivery_points d
    WHERE b.id=buyer AND s.id=seller AND p.id='c4cecebc-3d2d-5840-8021-57a9a11bc673' AND d.name='Rotterdam';
$$;

-- Bulk rows for the bound cases: one-tonne open ASKs of the owner, or
-- order-less trades with the owner as seller.
CREATE OR REPLACE FUNCTION offboarding_market_bulk(owner uuid, counterparty uuid, kind text, n integer)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    FOR i IN 1..n LOOP
        CASE kind
        WHEN 'orders' THEN PERFORM offboarding_market_order(gen_random_uuid(),owner,'ASK',1,1,'OPEN',NULL);
        WHEN 'pending' THEN PERFORM offboarding_market_trade(gen_random_uuid(),NULL,NULL,counterparty,owner,1,'PENDING_CONFIRMATION');
        WHEN 'confirmed' THEN PERFORM offboarding_market_trade(gen_random_uuid(),NULL,NULL,counterparty,owner,1,'CONFIRMED');
        ELSE RAISE EXCEPTION 'unknown bulk kind %', kind;
        END CASE;
    END LOOP;
END $$;

-- Rows keyed by identity, so a case can compare each row before and after.
CREATE OR REPLACE FUNCTION offboarding_market_snapshot() RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'orders',(SELECT coalesce(jsonb_object_agg(id,to_jsonb(o)),'{}') FROM orderbook_orders o),
        'trades',(SELECT coalesce(jsonb_object_agg(id,to_jsonb(t)),'{}') FROM trades t),
        'inventory',(SELECT coalesce(jsonb_object_agg(id,to_jsonb(i)),'{}') FROM inventory_items i),
        'audit',(SELECT coalesce(jsonb_agg(to_jsonb(a) ORDER BY id),'[]') FROM audit_logs a),
        'outbox',(SELECT coalesce(jsonb_agg(to_jsonb(e) ORDER BY id),'[]') FROM market_event_outbox e),
        'watch',(SELECT coalesce(jsonb_agg(to_jsonb(w) ORDER BY id),'[]') FROM watchlist_events w),
        'benchmarks',(SELECT coalesce(jsonb_agg(to_jsonb(b) ORDER BY id),'[]') FROM live_slice_benchmarks b));
$$;

-- A row that appears after discovery: the first slice lock of the session
-- that selects this schema first also records one more pending trade with
-- the subject as seller, then takes the real lock.
CREATE OR REPLACE FUNCTION offboarding_market_install_late_row(subject uuid, counterparty uuid)
RETURNS void LANGUAGE plpgsql AS $$ BEGIN
    CREATE SCHEMA offboarding_late_row;
    CREATE TABLE offboarding_late_row.plan(subject uuid NOT NULL, counterparty uuid NOT NULL, added boolean NOT NULL);
    INSERT INTO offboarding_late_row.plan VALUES(subject,counterparty,false);
    CREATE FUNCTION offboarding_late_row.pg_advisory_xact_lock(lock_key bigint) RETURNS void
        LANGUAGE plpgsql SECURITY DEFINER SET search_path=public,pg_catalog AS $body$
        DECLARE target offboarding_late_row.plan%ROWTYPE;
        BEGIN
            SELECT * INTO STRICT target FROM offboarding_late_row.plan;
            IF NOT target.added THEN
                PERFORM public.offboarding_market_trade(gen_random_uuid(),NULL,NULL,target.counterparty,target.subject,1,'PENDING_CONFIRMATION');
                UPDATE offboarding_late_row.plan SET added=true;
            END IF;
            PERFORM pg_catalog.pg_advisory_xact_lock(lock_key);
        END $body$;
    GRANT USAGE ON SCHEMA offboarding_late_row TO PUBLIC;
    GRANT EXECUTE ON FUNCTION offboarding_late_row.pg_advisory_xact_lock(bigint) TO PUBLIC;
END $$;
