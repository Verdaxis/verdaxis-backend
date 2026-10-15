-- Legacy curves accept any requested product UUID, without catalog joins.
-- Keep the common binder's parameter types, including unused surface filters.
WITH policy_products AS (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'products') AS product(
        id uuid, qualification_profile text, lanes jsonb
    )
), bindings AS (
    SELECT $4::boolean, $7::text, $8::text, $9::text, $10::text,
           $11::text, $12::text
)
SELECT o.*
FROM orderbook_orders AS o
WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
  AND (/* current_order_rule */)
  AND o.provenance = 'REAL'
  AND o.product_id = $5::uuid
  AND ($6::uuid IS NULL OR o.delivery_point_id = $6::uuid)
