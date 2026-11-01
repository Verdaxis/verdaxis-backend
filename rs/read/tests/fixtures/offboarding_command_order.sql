-- Test fixture for offboarding_command_postgres: one ASK order of ten tonnes in
-- organization $2, owned by user $3, for product $4 at delivery point $5,
-- with status $6. The remainder follows the order lifecycle check.
INSERT INTO orderbook_orders (
    id, organization_id, owner_user_id, product_id, delivery_point_id, side,
    quantity_mt, remaining_quantity_mt, price_per_mt_usd, status, provenance
) VALUES (
    $1, $2, $3, $4, $5, 'ASK', 10,
    CASE $6::text WHEN 'OPEN' THEN 10 WHEN 'PARTIALLY_FILLED' THEN 4 ELSE 0 END,
    500, $6::text, 'REAL'
)
