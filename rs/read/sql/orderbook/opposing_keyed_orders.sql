(
    SELECT b.*
    FROM orderbook_orders AS b
    WHERE EXISTS (
        SELECT 1
        FROM jsonb_array_elements($14::jsonb) AS k
        WHERE b.product_id = (k ->> 'product_id')::uuid
          AND b.delivery_point_id IS NOT DISTINCT FROM
              (k ->> 'delivery_point_id')::uuid
          AND b.availability_window = k ->> 'availability_window'
          AND b.provenance::text = k ->> 'provenance'
    )
)
