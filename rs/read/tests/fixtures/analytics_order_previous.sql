UPDATE orderbook_orders SET status = 'CANCELLED', availability_window = '2026-Q2'
WHERE created_at < '2026-06-01T00:00:00Z'::timestamptz;
