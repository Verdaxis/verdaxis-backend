-- Disposable settlement setup uses the normal migrated trade guards.
CREATE OR REPLACE FUNCTION settlement_fixture_confirmed(quantity numeric DEFAULT 3, price numeric DEFAULT 100, fee numeric DEFAULT 2, rate numeric DEFAULT 0) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 DELETE FROM trades WHERE id=source.id;
 source.quantity_mt:=quantity; source.price_per_mt_usd:=price;
 source.commission_fee_per_mt_usd:=fee; source.commission_rate_pct:=rate;
 source.commission_plan:=CASE WHEN fee IS NULL THEN NULL ELSE 'free' END;
 source.status:='CONFIRMED'; source.confirmed_at:=now();
 INSERT INTO trades SELECT source.*;
 UPDATE inventory_items SET reserved_stock_mt=7;
 RETURN source.id;
END $$;
CREATE OR REPLACE FUNCTION settlement_fixture_clone() RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 source.id:=gen_random_uuid();
 INSERT INTO trades SELECT source.*;
 RETURN source.id;
END $$;
CREATE OR REPLACE FUNCTION settlement_fixture_reject_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
 RAISE EXCEPTION 'audit unavailable' USING ERRCODE='23514';
END $$;

CREATE OR REPLACE FUNCTION settlement_fixture_reject_org(actor uuid) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 UPDATE organizations SET verification_status='REJECTED' WHERE id=(SELECT organization_id FROM users WHERE id=actor);
END $$;

CREATE OR REPLACE FUNCTION settlement_fixture_hold_key(actor uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE digest bytea; key bigint:=0; BEGIN
 digest:=sha256(convert_to(actor::text||'|trade.deliver|held','UTF8'));
 FOR i IN 0..7 LOOP key:=(key<<8)|get_byte(digest,i); END LOOP;
 PERFORM pg_advisory_xact_lock(CASE WHEN key=0 THEN 1 ELSE key END);
END $$;

CREATE OR REPLACE FUNCTION settlement_fixture_null_party(seller boolean) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades LIMIT 1;
 DELETE FROM trades WHERE id=source.id;
 IF seller THEN source.seller_user_id:=NULL; ELSE source.buyer_user_id:=NULL; END IF;
 INSERT INTO trades SELECT source.*;
END $$;


CREATE OR REPLACE FUNCTION settlement_fixture_assisted_side(side text, invalid_link boolean DEFAULT false) RETURNS uuid LANGUAGE plpgsql AS $$
DECLARE source orderbook_orders%ROWTYPE; trade trades%ROWTYPE; auth_id uuid:=gen_random_uuid(); staff uuid:='00000000-0000-0000-0000-000000004001'; customer uuid; BEGIN
 SELECT * INTO STRICT source FROM orderbook_orders LIMIT 1;
 SELECT * INTO STRICT trade FROM trades LIMIT 1;
 DELETE FROM trades; DELETE FROM orderbook_orders;
 customer:=CASE WHEN side='ASK' THEN trade.seller_id ELSE trade.buyer_id END;
 INSERT INTO market_support_authorizations(id,organization_id,accountable_user_id,order_side,product_id,delivery_point_id,availability_window,quantity_mt,price_per_mt_usd,certification_declared,certification_scheme,msds_available,authorization_expires_at,status,created_by_actor_user_id,terms_digest,evidence_reference,commercial_consent_version,commercial_consent_reference,idempotency_key,idempotency_request_hash)
 VALUES(auth_id,customer,staff,side,source.product_id,source.delivery_point_id,'SPOT',10,100,true,'ISCC EU',true,now()+interval '1 hour','CONSUMED',staff,repeat('a',64),'Fixture instruction','fixture-v1','Fixture instruction',auth_id::text,repeat('b',64));
 source.organization_id:=customer;source.side:=side;source.owner_user_id:=staff;source.created_by_actor_user_id:=staff;source.creation_method:='MARKET_SUPPORT';source.support_authorization_id:=auth_id;source.inventory_item_id:=NULL;
 INSERT INTO orderbook_orders SELECT source.*;
 trade.ask_order_id:=CASE WHEN side='ASK' THEN source.id ELSE NULL END;
 trade.bid_order_id:=CASE WHEN side='BID' THEN source.id ELSE NULL END;
 IF side='ASK' THEN trade.seller_user_id:=CASE WHEN invalid_link THEN '00000000-0000-0000-0000-000000002001'::uuid ELSE staff END;
 ELSE trade.buyer_user_id:=CASE WHEN invalid_link THEN '00000000-0000-0000-0000-000000001001'::uuid ELSE staff END; END IF;
 INSERT INTO trades SELECT trade.*;
 RETURN trade.id;
END $$;

-- Selective row visibility exercises a missing presence read without removing
-- retained parties or changing their foreign keys in the migrated schema.
CREATE OR REPLACE FUNCTION settlement_fixture_presence(table_name text, present boolean) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 IF table_name NOT IN ('users','organizations') THEN RAISE EXCEPTION 'unsupported fixture table'; END IF;
 IF present THEN
  EXECUTE format('DROP POLICY settlement_presence ON %I',table_name);
  EXECUTE format('ALTER TABLE %I DISABLE ROW LEVEL SECURITY',table_name);
 ELSE
  EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY',table_name);
  EXECUTE format('CREATE POLICY settlement_presence ON %I USING (position(%L in current_query())=0)',table_name,'SELECT EXISTS(SELECT 1 FROM '||table_name||' WHERE id=$1)');
 END IF;
END $$;

CREATE OR REPLACE FUNCTION settlement_fixture_hold_operation_key(actor uuid, operation text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE digest bytea; key bigint:=0; BEGIN
 digest:=sha256(convert_to(actor::text||'|'||operation||'|held','UTF8'));
 FOR i IN 0..7 LOOP key:=(key<<8)|get_byte(digest,i); END LOOP;
 PERFORM pg_advisory_xact_lock(CASE WHEN key=0 THEN 1 ELSE key END);
END $$;


CREATE OR REPLACE FUNCTION settlement_fixture_slow_receipt() RETURNS boolean LANGUAGE plpgsql VOLATILE AS $$ BEGIN
 PERFORM pg_sleep(6); RETURN true;
END $$;
CREATE OR REPLACE FUNCTION settlement_fixture_receipt_timeout(enabled boolean) RETURNS void LANGUAGE plpgsql AS $$ BEGIN
 IF enabled THEN
  ALTER TABLE market_command_results ENABLE ROW LEVEL SECURITY;
  CREATE POLICY settlement_receipt_wait ON market_command_results USING(settlement_fixture_slow_receipt());
 ELSE
  DROP POLICY settlement_receipt_wait ON market_command_results;
  ALTER TABLE market_command_results DISABLE ROW LEVEL SECURITY;
 END IF;
END $$;
