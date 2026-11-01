-- Installed only by the disposable reassignment target, after the trade,
-- confirmation and settlement fixtures. Fixture writes stay here, outside the
-- Rust sources the order-book write boundary scans.

-- People around the seller organization 2000 of the trade fixture.
CREATE OR REPLACE FUNCTION reassignment_fixture_people() RETURNS void LANGUAGE plpgsql AS $$
DECLARE seller uuid := '00000000-0000-0000-0000-000000002000'; other uuid := '00000000-0000-0000-0000-000000003000'; BEGIN
 -- The Python model loads only its current organization types.
 UPDATE organizations SET type='SHIPPING_LINE' WHERE type='SHIPOWNER';
 INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password,kyc_status,kyc_organization_id) VALUES
 ('00000000-0000-0000-0000-000000002002','handover-seller@example.test','unused','SUPPLIER','APPROVED',seller,true,false,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000002003','seller-buyer-role@example.test','unused','BUYER','APPROVED',seller,true,false,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000002004','seller-pending@example.test','unused','SUPPLIER','PENDING',seller,true,false,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000002005','seller-reset@example.test','unused','SUPPLIER','APPROVED',seller,true,true,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000002006','seller-unverified@example.test','unused','SUPPLIER','APPROVED',seller,false,false,'APPROVED',seller),
 ('00000000-0000-0000-0000-000000003002','other-supplier@example.test','unused','SUPPLIER','APPROVED',other,true,false,'APPROVED',other),
 ('00000000-0000-0000-0000-000000004002','second-admin@example.test','unused','ADMIN','APPROVED',NULL,true,false,'REJECTED',NULL);
END $$;

-- A copy of the base trade in the given lifecycle state.
CREATE OR REPLACE FUNCTION reassignment_fixture_trade(base uuid, state text) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades WHERE id=base;
 source.id:=gen_random_uuid();
 source.status:=state;
 source.created_at:=clock_timestamp();
 source.confirmed_at:=CASE WHEN state IN ('CONFIRMED','DELIVERED','PAID') THEN now() END;
 source.delivered_at:=CASE WHEN state IN ('DELIVERED','PAID') THEN now() END;
 source.paid_at:=CASE WHEN state='PAID' THEN now() END;
 source.final_quantity_mt:=CASE WHEN state IN ('DELIVERED','PAID') THEN source.quantity_mt END;
 source.final_price_per_mt:=CASE WHEN state IN ('DELIVERED','PAID') THEN source.price_per_mt_usd END;
 source.final_total_usd:=CASE WHEN state IN ('DELIVERED','PAID') THEN source.quantity_mt*source.price_per_mt_usd END;
 INSERT INTO trades SELECT source.*;
 RETURN source.id;
END $$;

-- An active freeze recorded by the first administrator.
CREATE OR REPLACE FUNCTION reassignment_fixture_freeze(target uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 INSERT INTO trade_freezes(id,trade_id,status,reason_code,note,frozen_by_user_id,frozen_at,version)
 VALUES(gen_random_uuid(),target,'ACTIVE','PAYMENT_DISPUTE','Fixture review','00000000-0000-0000-0000-000000004001',now(),1);
END $$;

-- What an unchecked reassignment would write: only the side's person changes.
CREATE OR REPLACE FUNCTION reassignment_fixture_force_side(target uuid, side text, person uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 IF side='SELLER' THEN UPDATE trades SET seller_user_id=person WHERE id=target;
 ELSIF side='BUYER' THEN UPDATE trades SET buyer_user_id=person WHERE id=target;
 ELSE RAISE EXCEPTION 'unknown fixture side'; END IF;
END $$;

-- One complete, comparable view of everything a reassignment could touch.
CREATE OR REPLACE FUNCTION reassignment_fixture_snapshot() RETURNS jsonb LANGUAGE sql AS $$
 SELECT jsonb_build_object(
 'trades',(SELECT jsonb_agg(to_jsonb(t) ORDER BY id) FROM trades t),
 'orders',(SELECT jsonb_agg(to_jsonb(o) ORDER BY id) FROM orderbook_orders o),
 'inventory',(SELECT jsonb_agg(to_jsonb(i) ORDER BY id) FROM inventory_items i),
 'users',(SELECT jsonb_agg(to_jsonb(u) ORDER BY id) FROM users u),
 'freezes',(SELECT jsonb_agg(to_jsonb(f) ORDER BY id) FROM trade_freezes f),
 'audit',(SELECT jsonb_agg(to_jsonb(a) ORDER BY id) FROM audit_logs a),
 'outbox',(SELECT jsonb_agg(jsonb_build_object('id',e.id,'event_type',e.event_type,'aggregate_type',e.aggregate_type,'aggregate_id',e.aggregate_id,'participants',e.participant_org_ids::jsonb,'payload',e.payload::jsonb) ORDER BY e.id) FROM market_event_outbox e),
 'receipts',(SELECT jsonb_agg(to_jsonb(r) ORDER BY id) FROM market_command_results r),
 'notifications',(SELECT count(*) FROM notifications));
$$;

-- A gate a test session closes and opens. A gated write waits for it inside
-- its own transaction, after that transaction has taken all its row locks.
-- The function's own lock bound replaces the caller's short one only while it
-- waits; the caller's statement bound still applies.
CREATE OR REPLACE FUNCTION reassignment_fixture_gate_close() RETURNS void LANGUAGE sql AS $$
 SELECT pg_advisory_lock(727700701);
$$;
CREATE OR REPLACE FUNCTION reassignment_fixture_gate_open() RETURNS boolean LANGUAGE sql AS $$
 SELECT pg_advisory_unlock(727700701);
$$;
CREATE OR REPLACE FUNCTION reassignment_fixture_gate_wait() RETURNS trigger LANGUAGE plpgsql SET lock_timeout='4s' AS $$ BEGIN
 PERFORM pg_advisory_xact_lock_shared(727700701);
 RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION reassignment_fixture_gate_install(target text) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 IF target='reassignment' THEN
  CREATE TRIGGER reassignment_fixture_gate BEFORE UPDATE OF buyer_user_id,seller_user_id ON trades FOR EACH ROW
  WHEN (OLD.buyer_user_id IS DISTINCT FROM NEW.buyer_user_id OR OLD.seller_user_id IS DISTINCT FROM NEW.seller_user_id)
  EXECUTE FUNCTION reassignment_fixture_gate_wait();
 ELSIF target='freeze' THEN
  CREATE TRIGGER reassignment_fixture_gate BEFORE INSERT ON trade_freezes FOR EACH ROW EXECUTE FUNCTION reassignment_fixture_gate_wait();
 ELSE RAISE EXCEPTION 'unknown fixture gate'; END IF;
END $$;
CREATE OR REPLACE FUNCTION reassignment_fixture_gate_remove() RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 DROP TRIGGER IF EXISTS reassignment_fixture_gate ON trades;
 DROP TRIGGER IF EXISTS reassignment_fixture_gate ON trade_freezes;
END $$;
