-- Rank the compatibility page first, then build the private digest input for
-- those rows only (Rust decision 24).
, page_keys AS MATERIALIZED (
    SELECT o.id, o.created_at
    FROM eligible_orders o
    ORDER BY o.created_at DESC, o.id
    OFFSET $14 LIMIT $15
)
SELECT detail.*, /* private_digest_input(base) */
FROM page_keys page
CROSS JOIN LATERAL (
    SELECT o.* FROM eligible_orders o WHERE o.id = page.id OFFSET 0
) detail
JOIN orderbook_orders base ON base.id = page.id
ORDER BY detail.created_at DESC, detail.id
