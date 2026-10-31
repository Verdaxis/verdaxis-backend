-- Installed only in fresh disposable databases by the residual writer coexistence suite.
-- Every row this suite needs is written here; the Rust target only calls these functions.

CREATE OR REPLACE FUNCTION residual_seed() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM publish_fixture_seed(0,1000);
    UPDATE products SET min_lot_size=1 WHERE id='c4cecebc-3d2d-5840-8021-57a9a11bc673';
    INSERT INTO users(id,email,password_hash,role,status,email_verified,must_change_password)
    VALUES('00000000-0000-0000-0000-000000003001','residual-admin@example.test','unused','ADMIN','APPROVED',true,false);
END $$;

-- Applies to sessions opened after the call: lock cycles are detected well inside
-- the 500 ms market lock bound instead of after the server default of one second.
CREATE OR REPLACE FUNCTION residual_fast_deadlock_detection() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE format('ALTER DATABASE %I SET deadlock_timeout = %L', current_database(), '100ms');
END $$;

CREATE OR REPLACE FUNCTION residual_deadlocks() RETURNS bigint LANGUAGE sql AS $$
    SELECT deadlocks FROM pg_stat_database WHERE datname=current_database()
$$;

-- The canonical slice key, computed exactly as both stacks compute it.
CREATE OR REPLACE FUNCTION residual_slice_key(product uuid, point uuid, win text) RETURNS bigint LANGUAGE sql AS $$
    SELECT ('x'||substr(encode(sha256(convert_to(product::text||'|'||coalesce(point::text,'')||'|'||win,'UTF8')),'hex'),1,16))::bit(64)::bigint
$$;

CREATE OR REPLACE FUNCTION residual_expire_at(target uuid, at_time timestamptz) RETURNS void LANGUAGE sql AS $$
    UPDATE orderbook_orders SET expires_at=at_time WHERE id=target;
$$;

-- A seller-owned executable REAL ASK without inventory in one B30 Rotterdam window.
CREATE OR REPLACE FUNCTION residual_ask(target uuid, price numeric, qty numeric, win text, expires timestamptz) RETURNS void LANGUAGE sql AS $$
    INSERT INTO orderbook_orders(id,organization_id,owner_user_id,created_by_actor_user_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,expires_at)
    SELECT target,'00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002001','00000000-0000-0000-0000-000000002001','SELF_SERVICE','REAL','ASK','c4cecebc-3d2d-5840-8021-57a9a11bc673',d.id,qty,qty,price,win,'OPEN',false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL',expires
    FROM delivery_points d WHERE d.name='Rotterdam';
$$;

-- A disclosed demo ASK of the demo activity seller in one B30 Rotterdam window.
CREATE OR REPLACE FUNCTION residual_demo_ask(target uuid, price numeric, win text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO organizations(id,name,domain,type,supplier_tier,verification_status,provenance)
    VALUES('d1e43e55-3fb0-4b5e-9f0b-93aa10050222','Verdaxis Demo Supplier Activity','demo-supplier.verdaxis.local','FUEL_SUPPLIER','REGIONAL_SUPPLIER','APPROVED','DEMO')
    ON CONFLICT(id) DO NOTHING;
    INSERT INTO orderbook_orders(id,organization_id,creation_method,provenance,side,product_id,delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,status,off_spec,certification_declared,certification_scheme,specification_standard,msds_available,carbon_intensity_gco2_mj,carbon_intensity_method,feedstock,origin,expires_at)
    SELECT target,'d1e43e55-3fb0-4b5e-9f0b-93aa10050222','SYSTEM','DEMO','ASK','c4cecebc-3d2d-5840-8021-57a9a11bc673',d.id,5,5,price,win,'OPEN',false,true,'ISCC EU','ISO 8217:2024 RF 380',true,10,'LIFECYCLE','residue','NL',now()+interval '2 days'
    FROM delivery_points d WHERE d.name='Rotterdam';
END $$;

CREATE OR REPLACE FUNCTION residual_order(target uuid) RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object('status',status::text,'remaining',remaining_quantity_mt::text,'expires_epoch',extract(epoch FROM expires_at)::text)
    FROM orderbook_orders WHERE id=target
$$;

-- Session advisory barrier: a Rust command whose pool carries the held
-- application name waits here at its first effect row, after its economic locks.
CREATE OR REPLACE FUNCTION residual_hold() RETURNS void LANGUAGE sql AS $$ SELECT pg_advisory_lock(120120001) $$;
CREATE OR REPLACE FUNCTION residual_release() RETURNS boolean LANGUAGE sql AS $$ SELECT pg_advisory_unlock(120120001) $$;
CREATE OR REPLACE FUNCTION residual_effect_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('application_name')='residual-rust-held' THEN
        -- The observation barrier is separate from the economic row-lock budget.
        PERFORM set_config('lock_timeout','10s',true);
        PERFORM pg_advisory_xact_lock(120120001);
        PERFORM set_config('lock_timeout','500ms',true);
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS residual_audit_barrier ON audit_logs;
DROP TRIGGER IF EXISTS residual_outbox_barrier ON market_event_outbox;
CREATE TRIGGER residual_audit_barrier BEFORE INSERT ON audit_logs FOR EACH ROW EXECUTE FUNCTION residual_effect_barrier();
CREATE TRIGGER residual_outbox_barrier BEFORE INSERT ON market_event_outbox FOR EACH ROW EXECUTE FUNCTION residual_effect_barrier();

-- The backend named `waiter` waiting on a heavyweight lock that a backend named `blocker` holds.
CREATE OR REPLACE FUNCTION residual_waiting(waiter text, blocker text) RETURNS integer LANGUAGE sql AS $$
    SELECT a.pid FROM pg_stat_activity a WHERE a.datname=current_database()
      AND a.application_name=waiter AND a.wait_event_type='Lock'
      AND EXISTS(SELECT 1 FROM pg_stat_activity b WHERE b.pid=ANY(pg_blocking_pids(a.pid)) AND b.application_name=blocker)
    LIMIT 1
$$;

-- Run on the barrier session: observe the wait, then release the barrier at once.
CREATE OR REPLACE FUNCTION residual_observe_release(waiter text, blocker text) RETURNS jsonb LANGUAGE plpgsql AS $$
DECLARE observed integer; deadline timestamptz:=clock_timestamp()+interval '20 seconds'; seen_at timestamptz;
BEGIN
    LOOP
        PERFORM pg_stat_clear_snapshot();
        observed:=residual_waiting(waiter,blocker);
        IF observed IS NOT NULL THEN
            seen_at:=clock_timestamp();
            PERFORM residual_release();
            RETURN jsonb_build_object('pid',observed,'release_ms',extract(epoch FROM clock_timestamp()-seen_at)*1000);
        END IF;
        IF clock_timestamp()>deadline THEN
            PERFORM residual_release();
            RAISE EXCEPTION '% did not wait on %', waiter, blocker;
        END IF;
        PERFORM pg_sleep(0.001);
    END LOOP;
END $$;

CREATE OR REPLACE FUNCTION residual_advisory_count(target integer) RETURNS bigint LANGUAGE sql AS $$
    SELECT count(*) FROM pg_locks WHERE pid=target AND locktype='advisory' AND granted
$$;

-- 'locked' when another transaction holds any row lock on the organization.
CREATE OR REPLACE FUNCTION residual_probe_organization(target uuid) RETURNS text LANGUAGE plpgsql AS $$
BEGIN
    PERFORM id FROM organizations WHERE id=target FOR UPDATE NOWAIT;
    RETURN 'free';
EXCEPTION WHEN lock_not_available THEN
    RETURN 'locked';
END $$;

CREATE OR REPLACE FUNCTION residual_hold_order(target uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM id FROM orderbook_orders WHERE id=target FOR UPDATE;
END $$;

-- The forbidden writer of the negative control: slice first, then a user row.
CREATE OR REPLACE FUNCTION residual_forbidden_slice(slice bigint) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(slice);
END $$;
CREATE OR REPLACE FUNCTION residual_forbidden_user(target uuid) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM id FROM users WHERE id=target FOR UPDATE;
END $$;

CREATE OR REPLACE FUNCTION residual_reminders() RETURNS jsonb LANGUAGE sql AS $$
    SELECT coalesce(jsonb_agg(jsonb_build_object(
        'id',id,'recipient',recipient_id,'order_id',data::jsonb->>'order_id',
        'expiry_epoch',data::jsonb->'expiry_epoch','status',data::jsonb->>'_email_status',
        'created_epoch',extract(epoch FROM created_at)::float8,'data',data::jsonb,'text',data::text)
        ORDER BY created_at,id),'[]'::jsonb)
    FROM notifications WHERE data::jsonb->>'event'='order_expiring'
$$;

CREATE OR REPLACE FUNCTION residual_invariants() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'stock',NOT EXISTS(SELECT 1 FROM inventory_items i WHERE current_stock_mt<0 OR reserved_stock_mt<0 OR incoming_stock_mt<0 OR scale(current_stock_mt)>2 OR scale(reserved_stock_mt)>2 OR scale(incoming_stock_mt)>2 OR reserved_stock_mt<>(
   COALESCE((SELECT sum(o.remaining_quantity_mt) FROM orderbook_orders o WHERE o.inventory_item_id=i.id AND o.status::text IN ('OPEN','PARTIALLY_FILLED')),0)+
   COALESCE((SELECT sum(t.quantity_mt) FROM trades t JOIN orderbook_orders o ON o.id=COALESCE(t.ask_order_id,t.bid_order_id) WHERE o.inventory_item_id=i.id AND t.status::text='PENDING_CONFIRMATION'),0))),
 'orders',NOT EXISTS(SELECT 1 FROM orderbook_orders o WHERE o.remaining_quantity_mt<0 OR o.remaining_quantity_mt>o.quantity_mt OR (o.status::text IN ('OPEN','PARTIALLY_FILLED') AND o.remaining_quantity_mt<>o.quantity_mt-COALESCE((SELECT sum(t.quantity_mt) FROM trades t WHERE (t.ask_order_id=o.id OR t.bid_order_id=o.id) AND t.status::text IN ('PENDING_CONFIRMATION','CONFIRMED','DELIVERED','PAID')),0))),
 'freeze',NOT EXISTS(SELECT 1 FROM trade_freezes GROUP BY trade_id HAVING count(*) FILTER(WHERE status='ACTIVE')>1));
$$;

-- Committed state digest: equal digests mean no row of these tables changed.
CREATE OR REPLACE FUNCTION residual_snapshot() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'orders',(SELECT md5(coalesce(string_agg(to_jsonb(o)::text,',' ORDER BY o.id),'')) FROM orderbook_orders o),
 'order_count',(SELECT count(*) FROM orderbook_orders),
 'trades',(SELECT md5(coalesce(string_agg(to_jsonb(t)::text,',' ORDER BY t.id),'')) FROM trades t),
 'trade_count',(SELECT count(*) FROM trades),
 'freezes',(SELECT md5(coalesce(string_agg(to_jsonb(f)::text,',' ORDER BY f.id),'')) FROM trade_freezes f),
 'inventory',(SELECT md5(coalesce(string_agg(to_jsonb(i)::text,',' ORDER BY i.id),'')) FROM inventory_items i),
 'notifications',(SELECT md5(coalesce(string_agg(to_jsonb(n)::text,',' ORDER BY n.id),'')) FROM notifications n),
 'negotiations',(SELECT md5(coalesce(string_agg(to_jsonb(n)::text,',' ORDER BY n.id),'')) FROM negotiations n),
 'audit_count',(SELECT count(*) FROM audit_logs),
 'outbox_count',(SELECT count(*) FROM market_event_outbox),
 'receipt_count',(SELECT count(*) FROM market_command_results),
 'quarantine_count',(SELECT count(*) FROM market_row_quarantines));
$$;

-- Demo activity rows and their outbox rows, as the documented columns.
CREATE OR REPLACE FUNCTION residual_demo_shape() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'orders',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'side',side::text,'organization_id',organization_id,'creation_method',creation_method::text,'provenance',provenance::text,'owner_user_id',owner_user_id,'created_by_actor_user_id',created_by_actor_user_id,'acceptance_ordinal',acceptance_ordinal,'status',status::text) ORDER BY acceptance_ordinal),'[]'::jsonb)
   FROM orderbook_orders WHERE organization_id IN ('7cc77115-0a9f-4ec4-8c74-05aa10050111','d1e43e55-3fb0-4b5e-9f0b-93aa10050222') AND creation_method::text='SYSTEM'),
 'trades',(SELECT coalesce(jsonb_agg(jsonb_build_object('id',id,'bid_order_id',bid_order_id,'ask_order_id',ask_order_id,'buyer_id',buyer_id,'seller_id',seller_id,'buyer_user_id',buyer_user_id,'seller_user_id',seller_user_id,'buyer_provenance',buyer_provenance::text,'seller_provenance',seller_provenance::text,'confirmed',confirmed_at IS NOT NULL,'status',status::text) ORDER BY created_at,id),'[]'::jsonb)
   FROM trades WHERE buyer_id='7cc77115-0a9f-4ec4-8c74-05aa10050111' AND seller_id='d1e43e55-3fb0-4b5e-9f0b-93aa10050222'),
 'outbox',(SELECT coalesce(jsonb_agg(jsonb_build_object('event_type',event_type,'participants',participant_org_ids::jsonb,'public',payload::jsonb->'_public_market_invalidation','evidence_absent',producer_evidence IS NULL) ORDER BY created_at,id),'[]'::jsonb)
   FROM market_event_outbox WHERE event_type='demo_market_changed'));
$$;

CREATE OR REPLACE FUNCTION residual_freeze(target uuid, cleared boolean) RETURNS void LANGUAGE plpgsql AS $$
DECLARE freeze uuid:=gen_random_uuid();
BEGIN
    INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id)
    VALUES(freeze,target,'ACTIVE','COMPLIANCE_REVIEW','Fixture review','00000000-0000-0000-0000-000000003001');
    IF cleared THEN
        UPDATE trade_freezes SET status='CLEARED',cleared_at=now(),cleared_by_user_id='00000000-0000-0000-0000-000000003001',clear_note='Fixture clearance',version=2 WHERE id=freeze;
    END IF;
END $$;

-- Moves demo activity trades behind the seven-day retention cutoff.
CREATE OR REPLACE FUNCTION residual_age_demo_trades(days integer) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('session_replication_role','replica',true);
    UPDATE trades SET created_at=now()-make_interval(days=>days)
    WHERE buyer_id='7cc77115-0a9f-4ec4-8c74-05aa10050111' AND seller_id='d1e43e55-3fb0-4b5e-9f0b-93aa10050222';
    PERFORM set_config('session_replication_role','origin',true);
END $$;

CREATE OR REPLACE FUNCTION residual_seed_shape() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'demo_orders',(SELECT count(*) FROM orderbook_orders WHERE provenance::text='DEMO'),
 'demo_orders_with_owner',(SELECT count(*) FROM orderbook_orders WHERE provenance::text='DEMO' AND (owner_user_id IS NOT NULL OR created_by_actor_user_id IS NOT NULL)),
 'demo_trades',(SELECT count(*) FROM trades WHERE buyer_provenance::text='DEMO' AND seller_provenance::text='DEMO'),
 'demo_trades_with_users',(SELECT count(*) FROM trades WHERE buyer_provenance::text='DEMO' AND seller_provenance::text='DEMO' AND (buyer_user_id IS NOT NULL OR seller_user_id IS NOT NULL)),
 'demo_trades_linked',(SELECT count(*) FROM trades WHERE buyer_provenance::text='DEMO' AND seller_provenance::text='DEMO' AND bid_order_id IS NOT NULL AND ask_order_id IS NOT NULL),
 'real_orders',(SELECT count(*) FROM orderbook_orders WHERE provenance::text='REAL'));
$$;

-- Phase 7 rows that reference order-book rows (inserted with triggers off: the
-- rows only need to exist and reference the orders).
CREATE OR REPLACE FUNCTION residual_phase7_references(bid uuid, ask uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE negotiation uuid:=gen_random_uuid();
BEGIN
    PERFORM set_config('session_replication_role','replica',true);
    INSERT INTO negotiations(id,bid_order_id,ask_order_id,initiator_org_id,counterparty_org_id,initiator_user_id,counterparty_user_id,initiator_side,product_id,delivery_point_id,availability_window,quantity_mt,current_price,status,last_actor_org_id,expires_at,created_at,updated_at)
    SELECT negotiation,bid,ask,b.organization_id,a.organization_id,b.owner_user_id,a.owner_user_id,'BUYER',a.product_id,a.delivery_point_id,a.availability_window,1,100,'OPEN',b.organization_id,now()+interval '1 day',now(),now()
    FROM orderbook_orders a, orderbook_orders b WHERE a.id=ask AND b.id=bid;
    INSERT INTO negotiation_rounds(id,negotiation_id,round_number,proposer_org_id,proposer_user_id,proposed_price,notes,created_at)
    SELECT gen_random_uuid(),negotiation,1,initiator_org_id,initiator_user_id,100,'Fixture round',now() FROM negotiations WHERE id=negotiation;
    INSERT INTO supplier_offers(id,supplier_org_id,supplier_user_id,product_id,delivery_point_id,quantity_mt,min_fill_mt,price_per_mt_usd,availability_window,listing_terms,status,revision,expires_at,created_at,updated_at)
    SELECT gen_random_uuid(),'00000000-0000-0000-0000-000000002000','00000000-0000-0000-0000-000000002001',p.id,d.id,100,10,100,'SPOT','{}','OPEN',1,now()+interval '1 day',now(),now()
    FROM products p, delivery_points d WHERE p.name='UCOME B100' AND d.name='Singapore';
    PERFORM set_config('session_replication_role','origin',true);
END $$;

CREATE OR REPLACE FUNCTION residual_phase7_state() RETURNS jsonb LANGUAGE sql AS $$
SELECT jsonb_build_object(
 'negotiations',(SELECT coalesce(jsonb_agg(to_jsonb(n) ORDER BY n.id),'[]'::jsonb) FROM negotiations n),
 'rounds',(SELECT coalesce(jsonb_agg(to_jsonb(r) ORDER BY r.id),'[]'::jsonb) FROM negotiation_rounds r),
 'offers',(SELECT coalesce(jsonb_agg(to_jsonb(s) ORDER BY s.id),'[]'::jsonb) FROM supplier_offers s),
 'referenced_orders',(SELECT count(*) FROM orderbook_orders o WHERE EXISTS(SELECT 1 FROM negotiations n WHERE n.bid_order_id=o.id OR n.ask_order_id=o.id)));
$$;
