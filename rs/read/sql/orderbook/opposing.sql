SELECT keys.slice,
       CASE WHEN min(o.side::text) = 'ASK' THEN min(o.price_per_mt_usd)
            ELSE max(o.price_per_mt_usd) END AS best_price
FROM eligible_orders o
JOIN jsonb_array_elements($14::jsonb) AS keys(slice)
  ON o.product_id = (keys.slice->>'product_id')::uuid
 AND o.delivery_point_id IS NOT DISTINCT FROM (keys.slice->>'delivery_point_id')::uuid
 AND o.availability_window = keys.slice->>'availability_window'
 AND o.provenance::text = keys.slice->>'provenance'
GROUP BY keys.slice
