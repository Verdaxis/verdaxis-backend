-- Rank narrow eligible keys, then build the private digest input for the page
-- rows only (Rust decision 24). The base join is one primary-key lookup per
-- returned row; the shared digest definition is expanded by the caller.
, page_keys AS MATERIALIZED (
    SELECT o.id, o.price_per_mt_usd, o.acceptance_ordinal,
           o.remaining_quantity_mt, o.created_at
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
)
SELECT detail.*, /* private_digest_input(base) */, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label
FROM page_keys page
CROSS JOIN LATERAL (
    SELECT o.* FROM eligible_orders o WHERE o.id = page.id OFFSET 0
) detail
JOIN orderbook_orders base ON base.id = page.id
LEFT JOIN organizations org ON org.id = detail.organization_id
ORDER BY
    CASE WHEN $14 = 'price_asc' THEN page.price_per_mt_usd END ASC,
    CASE WHEN $14 = 'price_desc' THEN page.price_per_mt_usd END DESC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.acceptance_ordinal END ASC,
    CASE WHEN $14 IN ('price_asc', 'price_desc') THEN page.id END ASC,
    CASE WHEN $14 = 'quantity_desc' THEN page.remaining_quantity_mt END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.created_at END DESC,
    CASE WHEN $14 IN ('newest', 'quantity_desc') THEN page.id END DESC
