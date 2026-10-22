-- Task 9: restrict the base order scan to requested curve cells before the
-- shared eligibility predicate runs.
--
-- A semi-join, not a join: a caller may repeat a key or send both a
-- null-window key and a specific-window key for the same slice (the board does
-- both), and a join would then admit one order twice. `EXISTS` admits each
-- base row at most once, so the row multiset is exactly the one the full
-- relation produces under the same tail. The eligibility definition applied
-- afterwards is the shared one; nothing here duplicates a predicate.
SELECT o.* FROM orderbook_orders AS o
WHERE EXISTS (
    SELECT 1
    FROM requested_curve_slices AS s
    WHERE s.product_id = o.product_id
      AND s.delivery_point_id = o.delivery_point_id
      AND (s.availability_window IS NULL OR s.availability_window = o.availability_window)
)
