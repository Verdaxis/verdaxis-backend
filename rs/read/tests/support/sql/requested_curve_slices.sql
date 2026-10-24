-- Task 9: the requested curve cells for one statement.
--
-- `$14` is the caller's key list: catalog product id, delivery point id and
-- availability window. A NULL window means "every window for that product and
-- point" (the board's focus key); every other key names one window. The list
-- is materialised once, so the base scan and any later consumer read it once.
SELECT DISTINCT product_id, delivery_point_id, availability_window
FROM jsonb_to_recordset($14::jsonb) AS k(
    product_id uuid,
    delivery_point_id uuid,
    availability_window text
)
