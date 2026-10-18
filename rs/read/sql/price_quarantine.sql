-- Metadata only: no price, quantity or party identity leaves this relation.
SELECT product_id, product_name, fuel_type, fuel_grade, market_product,
       availability_window, delivery_point_id, delivery_point_name,
       delivery_point_region, confirmed_at
FROM formal_trades
WHERE coalesce(buyer_provenance, 'UNKNOWN') NOT IN ('TEST', 'CANARY')
  AND coalesce(seller_provenance, 'UNKNOWN') NOT IN ('TEST', 'CANARY')
  AND (coalesce(buyer_provenance, 'UNKNOWN') = 'UNKNOWN'
       OR coalesce(seller_provenance, 'UNKNOWN') = 'UNKNOWN'
       OR buyer_provenance <> seller_provenance)
