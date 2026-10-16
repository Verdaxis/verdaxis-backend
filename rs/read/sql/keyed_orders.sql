-- Rust decision 27: restrict the base order scan to rows carrying a requested
-- benchmark key BEFORE the shared eligibility predicate, so eligibility runs
-- only for keyed rows (as Python's index scan does). All four key columns
-- participate, so each keyed order enters once. The eligibility definition
-- itself is unchanged and is applied by the shared relation afterwards.
SELECT o.* FROM orderbook_orders AS o
JOIN requested_slices AS s
  ON s.side = o.side
 AND s.product_id = o.product_id
 AND s.point = o.delivery_point_id
 AND s."window" = o.availability_window
