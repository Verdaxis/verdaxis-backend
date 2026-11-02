-- Watchlist event admission fixtures, run by the schema owner on a migrated
-- disposable database. One REAL organization holds an open BID in the public
-- scope and a cancelled BID outside it, both in one SPOT slice. The owner's
-- watchlist has two slices and a pin on each order.
CREATE OR REPLACE FUNCTION producer_visibility_seed(
    product uuid, product_name text, fuel_type text, fuel_grade text,
    point uuid, point_name text, region text,
    org uuid, owner uuid, public_order uuid, closed_order uuid,
    container uuid, spot_slice uuid, later_slice uuid, public_pin uuid, closed_pin uuid,
    code text
) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO products(id,name,fuel_type,fuel_grade,is_active)
    VALUES(product,product_name,fuel_type,fuel_grade,true);
    INSERT INTO delivery_points(id,name,region,timezone,is_active)
    VALUES(point,point_name,region,'UTC',true);
    INSERT INTO organizations(id,name,type) VALUES(org,'Visibility organization '||org::text,'FUEL_BUYER');
    -- Company approval is the trust decision that classifies it as REAL.
    UPDATE organizations SET verification_status='APPROVED' WHERE id=org;
    INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password)
    VALUES(owner,owner::text||'@example.test','unused','BUYER','APPROVED',org,true,false);
    INSERT INTO orderbook_orders(id,organization_id,product_id,delivery_point_id,provenance,side,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status)
    VALUES(public_order,org,product,point,'REAL','BID',10,10,500,'SPOT','OPEN'),
          (closed_order,org,product,point,'REAL','BID',10,10,500,'SPOT','CANCELLED');
    INSERT INTO watchlists(id,user_id,name,kind,created_at)
    VALUES(container,owner,'Producer visibility','CUSTOM','2026-10-01T00:00:00Z');
    INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,availability_window_code,order_id,created_at)
    VALUES(spot_slice,container,'SLICE',code,point,'SPOT',NULL,'2026-10-01T00:01:00Z'),
          (later_slice,container,'SLICE',code,point,'2027-01',NULL,'2026-10-01T00:02:00Z'),
          (public_pin,container,'PIN',code,point,'SPOT',public_order,'2026-10-01T00:03:00Z'),
          (closed_pin,container,'PIN',code,point,'SPOT',closed_order,'2026-10-01T00:04:00Z');
END $$;

-- One stored event. A trusted row carries the producer identity the trusted
-- producer writes: a source mutation, its sequence, the attestation instant
-- and a derived key. `derived_key` names the transition within its source, so
-- two sources may carry the same key. A legacy row takes the column defaults.
CREATE OR REPLACE FUNCTION producer_visibility_event(
    event_id uuid, container uuid, target uuid, kind text, source_order uuid,
    created timestamptz, read boolean, trusted boolean, attested timestamptz, derived_key text
) RETURNS void LANGUAGE plpgsql AS $$
DECLARE payload json;
BEGIN
    IF source_order IS NULL THEN
        payload := json_build_object('schema_version',1,'source_kind','LIVE_ORDER');
    ELSE
        payload := json_build_object('schema_version',1,'source_kind','LIVE_ORDER','order_id',source_order::text);
    END IF;
    IF trusted THEN
        INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at,
                                     producer,source_event_id,source_stream_seq,scope_attested_at,derived_event_key)
        VALUES(event_id,container,target,kind,payload,read,created,
               'rust-market-v1',gen_random_uuid(),1,attested,kind||'|'||derived_key||'|0');
    ELSE
        INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,event_payload,is_read,created_at)
        VALUES(event_id,container,target,kind,payload,read,created);
    END IF;
END $$;

-- The open order leaves the public scope.
CREATE OR REPLACE FUNCTION producer_visibility_close_order(ord uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('session_replication_role','replica',true);
    UPDATE orderbook_orders SET status='CANCELLED' WHERE id=ord;
    PERFORM set_config('session_replication_role','origin',true);
END $$;

-- A paused trusted producer: its watermark row exists and nothing consumes.
CREATE OR REPLACE FUNCTION producer_visibility_pause() RETURNS void LANGUAGE sql AS $$
    INSERT INTO watchlist_producer_state(singleton,consumed_through_seq,updated_at)
    VALUES(true,1,'2026-10-20T00:00:00Z')
$$;

-- Every stored event's identity, read state and row version, and the producer
-- watermark row, for a test that proves a refused or read-only path wrote
-- nothing.
CREATE OR REPLACE FUNCTION producer_visibility_rows(container uuid) RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'events', (SELECT coalesce(jsonb_object_agg(id::text, jsonb_build_array(is_read, xmin::text)), '{}'::jsonb)
                   FROM watchlist_events WHERE watchlist_id=container),
        'producer', (SELECT to_jsonb(s) || jsonb_build_object('xmin', s.xmin::text) FROM watchlist_producer_state s))
$$;
