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
