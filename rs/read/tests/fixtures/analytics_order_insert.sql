-- Test fixture: one order-book row in the disposable analytics schema (Task 19).
-- The provenance label is explicit so each case can build live, demo and
-- unknown rows deliberately instead of inheriting one default.
INSERT INTO orderbook_orders(id,organization_id,side,provenance,product_id,delivery_point_id,availability_window,remaining_quantity_mt,price_per_mt_usd,status,created_at) VALUES($1,$2,$3,$9::text,$4,$5,'SPOT',$7,$6,'OPEN',$8)
