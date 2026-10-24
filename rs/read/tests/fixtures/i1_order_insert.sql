-- Test fixture: one open ASK row for the integration composition case.
INSERT INTO orderbook_orders(id,organization_id,side,product_id,delivery_point_id,availability_window,remaining_quantity_mt,inventory_item_id) VALUES($1,$2,'ASK',$3,$4,'SPOT',60,$5)
