-- Test fixture: the order-row lock a Python trade-lifecycle request takes after
-- its market-slice lock (app/routers/trades.py, `_load_trade` followed by the
-- linked-order `with_for_update` select). The statement lives here so no Rust
-- literal outside the write boundary names an order-book row lock.
SELECT id FROM orderbook_orders WHERE id = $1 FOR UPDATE
