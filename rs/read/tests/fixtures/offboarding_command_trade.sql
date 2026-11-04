-- Test fixture for offboarding_command_postgres: one ten-tonne SPOT trade
-- between buyer organization $2 and seller organization $3, with user $4 as
-- the recorded seller, at status $5. $6 is a canonical product and $7 a
-- canonical delivery point; the snapshot labels are copied from those rows,
-- and the lifecycle timestamps and final values follow the status.
INSERT INTO trades (
    id, buyer_id, seller_id, seller_user_id, initiator_org_id, initiated_by,
    buyer_provenance, seller_provenance, quantity_mt, price_per_mt_usd, status,
    confirmed_at, delivered_at, paid_at, final_quantity_mt, final_price_per_mt,
    final_total_usd, market_snapshot_version, product_id, product_name, fuel_type,
    fuel_grade, market_product, availability_window, delivery_point_id,
    delivery_point_name, delivery_point_region
)
SELECT
    $1, $2, $3, $4, $2, 'BUYER', 'REAL', 'REAL', 10, 500, $5::text,
    CASE WHEN $5::text IN ('CONFIRMED', 'DELIVERED', 'PAID') THEN now() END,
    CASE WHEN $5::text IN ('DELIVERED', 'PAID') THEN now() END,
    CASE WHEN $5::text = 'PAID' THEN now() END,
    CASE WHEN $5::text IN ('DELIVERED', 'PAID') THEN 10 END,
    CASE WHEN $5::text IN ('DELIVERED', 'PAID') THEN 500 END,
    CASE WHEN $5::text IN ('DELIVERED', 'PAID') THEN 5000 END,
    1, p.id, p.name, p.fuel_type, p.fuel_grade, 'BIO_METHANOL', 'SPOT', d.id, d.name, d.region
FROM products p, delivery_points d
WHERE p.id = $6 AND d.id = $7
