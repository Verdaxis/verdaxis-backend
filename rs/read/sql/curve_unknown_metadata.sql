-- Metadata only: current UNKNOWN orders never supply economic values.
-- Uses the same captured instant/date and KYC switch as the economic relation.
WITH policy_products AS (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'products') AS p(
        id uuid, qualification_profile text, specification_standard text, lanes jsonb
    )
)
/* eligibility_sets */
SELECT o.product_id, o.delivery_point_id, o.availability_window, count(*)::bigint AS unknown_order_count
FROM orderbook_orders o
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
  AND (/* current_order_rule */)
  AND o.provenance = 'UNKNOWN'
  AND EXISTS (
      SELECT 1 FROM jsonb_to_recordset($4::jsonb) AS k(product_id uuid, delivery_point_id uuid, availability_window text)
      WHERE k.product_id = o.product_id AND k.delivery_point_id = o.delivery_point_id
        AND (k.availability_window IS NULL OR k.availability_window = o.availability_window)
  )
GROUP BY o.product_id, o.delivery_point_id, o.availability_window
