(
    SELECT b.*
    FROM orderbook_orders AS b
    WHERE (
        b.product_id,
        b.delivery_point_id,
        b.availability_window,
        b.provenance::text
    ) IN (
        SELECT (k ->> 'product_id')::uuid,
               (k ->> 'delivery_point_id')::uuid,
               k ->> 'availability_window',
               k ->> 'provenance'
        FROM jsonb_array_elements($14::jsonb) AS k
    )
)
