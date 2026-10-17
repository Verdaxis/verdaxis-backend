, ranked_orders AS (
    SELECT o.*, row_number() OVER (
        PARTITION BY o.side
        ORDER BY
            CASE WHEN o.side = 'BID' THEN o.price_per_mt_usd END DESC,
            CASE WHEN o.side = 'ASK' THEN o.price_per_mt_usd END ASC,
            o.acceptance_ordinal ASC,
            o.id ASC
    ) AS depth_rank
    FROM eligible_orders o
)
SELECT o.*, /* private_digest_input(base) */, COALESCE(org.supplier_tier, 'INDEPENDENT') AS tier_label
FROM ranked_orders o
JOIN orderbook_orders base ON base.id = o.id
LEFT JOIN organizations org ON org.id = o.organization_id
WHERE o.depth_rank <= 15
ORDER BY o.side, o.depth_rank
