-- Disposable lifecycle setup preserves the migrated immutable trade guards.
CREATE OR REPLACE FUNCTION membership_fixture_pending(target uuid) RETURNS void LANGUAGE plpgsql AS $$
DECLARE source trades%ROWTYPE; BEGIN
 SELECT * INTO STRICT source FROM trades WHERE id=target;
 DELETE FROM trades WHERE id=target;
 source.status:='PENDING_CONFIRMATION'; source.confirmed_at:=NULL;
 INSERT INTO trades SELECT source.*;
 UPDATE inventory_items SET reserved_stock_mt=10;
END $$;
