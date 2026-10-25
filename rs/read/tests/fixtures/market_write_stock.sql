-- Minimal shape of the existing shared rows. Unconstrained NUMERIC permits
-- corruption fixtures; the ledger must refuse them before attempting a write.
CREATE TABLE inventory_items (
    id uuid PRIMARY KEY,
    current_stock_mt numeric NOT NULL,
    reserved_stock_mt numeric NOT NULL,
    incoming_stock_mt numeric NOT NULL
);
CREATE TABLE orderbook_orders (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL,
    inventory_item_id uuid REFERENCES inventory_items(id) ON DELETE RESTRICT,
    product_id uuid NOT NULL,
    delivery_point_id uuid,
    availability_window text NOT NULL,
    side text NOT NULL,
    status text NOT NULL,
    quantity_mt numeric NOT NULL,
    remaining_quantity_mt numeric NOT NULL,
    version integer NOT NULL DEFAULT 1,
    expires_at timestamptz
);
CREATE TABLE trades (
    id uuid PRIMARY KEY,
    bid_order_id uuid REFERENCES orderbook_orders(id),
    ask_order_id uuid REFERENCES orderbook_orders(id),
    quantity_mt numeric NOT NULL,
    status text NOT NULL
);
CREATE FUNCTION seed_stock(item uuid, ord uuid, product uuid, org uuid, n integer)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE i integer; item_id uuid; order_id uuid;
BEGIN
    FOR i IN 0..n-1 LOOP
        item_id := (lpad(to_hex(i+1),32,'0'))::uuid;
        order_id := (lpad(to_hex(i+10000),32,'0'))::uuid;
        IF i=0 THEN order_id := ord; END IF;
        INSERT INTO inventory_items VALUES(item_id,10,0,0);
        INSERT INTO orderbook_orders(id,organization_id,inventory_item_id,product_id,
            availability_window,side,status,quantity_mt,remaining_quantity_mt,expires_at)
        VALUES(order_id,org,item_id,product,'SPOT','ASK','OPEN',10,10,'2026-10-25T11:00:00Z');
    END LOOP;
END $$;
CREATE FUNCTION stage_stock(ord uuid, action text, q numeric)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF action='pending' THEN
        INSERT INTO trades VALUES(gen_random_uuid(),NULL,ord,q,'PENDING_CONFIRMATION');
        UPDATE orderbook_orders SET remaining_quantity_mt=remaining_quantity_mt-q,
            status=CASE WHEN remaining_quantity_mt=q THEN 'FILLED' ELSE 'PARTIALLY_FILLED' END
            WHERE id=ord;
    ELSIF action='confirm' THEN
        UPDATE trades SET status='CONFIRMED' WHERE ask_order_id=ord AND status='PENDING_CONFIRMATION';
    ELSE
        UPDATE trades SET status=CASE WHEN action='cancel_trade' THEN 'CANCELLED' ELSE 'DECLINED' END
            WHERE ask_order_id=ord AND status='PENDING_CONFIRMATION';
    END IF;
END $$;
CREATE FUNCTION corrupt_stock(item uuid, field text, q numeric)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE format('UPDATE inventory_items SET %I=$1 WHERE id=$2',field) USING q,item;
END $$;
CREATE FUNCTION reset_stock(item uuid)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    UPDATE inventory_items SET current_stock_mt=10,reserved_stock_mt=0,incoming_stock_mt=0 WHERE id=item;
END $$;

CREATE FUNCTION seed_stock_history(item uuid, ord uuid, n integer)
RETURNS void LANGUAGE plpgsql AS $$
DECLARE i integer; history_id uuid; live_id uuid;
BEGIN
    UPDATE orderbook_orders SET remaining_quantity_mt=6,status='PARTIALLY_FILLED' WHERE id=ord;
    INSERT INTO trades VALUES(gen_random_uuid(),NULL,ord,4,'PENDING_CONFIRMATION');
    FOR i IN 1..n LOOP
        history_id := gen_random_uuid(); live_id := gen_random_uuid();
        INSERT INTO orderbook_orders
            SELECT history_id,organization_id,item,product_id,NULL,'SPOT','ASK','CANCELLED',10,10,1,NULL
            FROM orderbook_orders WHERE id=ord;
        INSERT INTO trades VALUES(gen_random_uuid(),NULL,history_id,4,'CANCELLED');
        INSERT INTO orderbook_orders
            SELECT live_id,organization_id,item,product_id,NULL,'SPOT','ASK','PARTIALLY_FILLED',0.02,0.01,1,NULL
            FROM orderbook_orders WHERE id=ord;
        INSERT INTO trades VALUES(gen_random_uuid(),NULL,live_id,0.01,'CONFIRMED');
    END LOOP;
    UPDATE inventory_items SET current_stock_mt=0,reserved_stock_mt=10+n*0.01 WHERE id=item;
END $$;
