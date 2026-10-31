-- Test fixture: one open ASK row in the disposable schema (Task 7 suite).
INSERT INTO orderbook_orders(id,organization_id,side,product_id,delivery_point_id,availability_window,remaining_quantity_mt,inventory_item_id) VALUES($1,$2,'ASK',$3,$4,'SPOT',$5,$6)
