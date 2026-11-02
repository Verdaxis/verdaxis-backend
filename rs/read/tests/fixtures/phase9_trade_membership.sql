-- Disposable lifecycle setup preserves the migrated immutable trade guards.
CREATE OR REPLACE FUNCTION membership_fixture_pending(target uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades WHERE id=target;
 DELETE FROM trades WHERE id=target;
 source.status:='PENDING_CONFIRMATION'; source.confirmed_at:=NULL;
 INSERT INTO trades SELECT source.*;
 UPDATE inventory_items SET reserved_stock_mt=10;
END $$;

CREATE OR REPLACE FUNCTION membership_fixture_inconsistent(target uuid, kind text) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 IF kind='slice' THEN
  SELECT * INTO STRICT source FROM trades WHERE id=target;
  DELETE FROM trades WHERE id=target;
  source.availability_window:='2027-01';
  INSERT INTO trades SELECT source.*;
 ELSIF kind='inventory' THEN
  ALTER TABLE inventory_items ENABLE ROW LEVEL SECURITY;
  CREATE POLICY membership_inventory_visibility ON inventory_items USING (false);
 ELSE RAISE EXCEPTION 'unsupported fixture state'; END IF;
END $$;
