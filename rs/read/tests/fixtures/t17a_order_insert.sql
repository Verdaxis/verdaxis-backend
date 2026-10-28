-- Test fixture: one order-book row for the product-usage aggregate cases.
-- The write lives here so no Rust literal outside `order_write` names an
-- order-book DML target.
INSERT INTO orderbook_orders(id,organization_id,created_at) VALUES($1,$2,$3)
