-- Test fixture: one order-book row in the disposable analytics schema (Task 19).
INSERT INTO orderbook_orders(id,organization_id,side,provenance,product_id,delivery_point_id,availability_window,remaining_quantity_mt,price_per_mt_usd,status,created_at) VALUES($1,$2,$3,'REAL',$4,$5,'SPOT',$7,$6,'OPEN',$8)
