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
