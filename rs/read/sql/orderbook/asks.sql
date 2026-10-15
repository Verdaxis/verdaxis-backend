SELECT o.*, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label, count(*) OVER () AS total
FROM eligible_orders o
LEFT JOIN organizations org ON org.id = o.organization_id
ORDER BY
    CASE WHEN $14 = 'price_asc' THEN o.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN o.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN o.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.id END DESC
LIMIT $15 OFFSET $16
