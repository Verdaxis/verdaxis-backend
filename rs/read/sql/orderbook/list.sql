SELECT page.*, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label
FROM (
SELECT o.*
FROM eligible_orders o
ORDER BY
    CASE WHEN $14 = 'price_asc' THEN o.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN o.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN o.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN o.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN o.id END DESC
LIMIT $15 OFFSET $16
) AS page
LEFT JOIN organizations org ON org.id = page.organization_id

ORDER BY
    CASE WHEN $14 = 'price_asc' THEN page.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN page.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN page.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.id END DESC
