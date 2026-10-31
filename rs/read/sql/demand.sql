WITH policy_products AS MATERIALIZED (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'products') AS p(
        id uuid, code text, name text, fuel_type text, fuel_grade text,
        execution_mode text, lanes jsonb, qualification_profile text,
        specification_standard text
    )
), policy_points AS MATERIALIZED (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'points') AS p(
        id uuid, name text, region text
    )
)
/* eligibility_sets */
, current_bids AS NOT MATERIALIZED (
    SELECT pp.code AS market_product, dp.id AS delivery_point_id,
           dp.name AS delivery_point_name, dp.region,
           o.availability_window, o.provenance,
           o.remaining_quantity_mt, o.price_per_mt_usd, o.created_at
    FROM orderbook_orders o
    JOIN products p ON p.id = o.product_id AND p.is_active
    JOIN policy_products pp ON pp.id = p.id AND pp.name = p.name
        AND pp.fuel_type = p.fuel_type AND pp.fuel_grade = p.fuel_grade
        AND pp.execution_mode = 'ORDERBOOK'
    JOIN delivery_points dp ON dp.id = o.delivery_point_id AND dp.is_active
    JOIN policy_points pq ON pq.id = dp.id AND pq.name = dp.name AND pq.region = dp.region
    WHERE o.side = 'BID' AND o.status IN ('OPEN', 'PARTIALLY_FILLED')
      AND o.provenance IN ('REAL', 'DEMO', 'UNKNOWN')
      AND EXISTS (SELECT 1 FROM jsonb_array_elements_text(pp.lanes) lane(value)
                  WHERE lane.value = dp.id::text)
      AND (/* current_order_rule */)
      AND (o.availability_window = 'SPOT' OR (
          o.availability_window ~ '^[0-9]{4}-(0[1-9]|1[0-2]|Q[1-4]|CAL)$'
          AND left(o.availability_window, 4) <> '0000'
      ))
      AND ($4::text IS NULL OR p.fuel_type ILIKE '%' || $4 || '%' OR p.name ILIKE '%' || $4 || '%')
      AND ($7::text IS NULL OR dp.region ILIKE '%' || $7 || '%')
), candidate_groups AS MATERIALIZED (
    SELECT market_product, delivery_point_id, delivery_point_name, region,
           availability_window, provenance
    FROM current_bids
    GROUP BY market_product, delivery_point_id, delivery_point_name, region,
             availability_window, provenance
    LIMIT 32769
), guard AS MATERIALIZED (
    SELECT count(*) > 32768 AS exceeded FROM candidate_groups
), grouped AS (
    SELECT market_product, delivery_point_id, delivery_point_name, region,
           availability_window, provenance,
           CASE WHEN provenance <> 'UNKNOWN' THEN sum(remaining_quantity_mt) END AS volume_mt,
           CASE WHEN provenance <> 'UNKNOWN' THEN max(price_per_mt_usd) END AS max_price_per_mt,
           count(*)::bigint AS order_count, max(created_at) AS created_at
    FROM current_bids
    WHERE NOT (SELECT exceeded FROM guard)
    GROUP BY market_product, delivery_point_id, delivery_point_name, region,
             availability_window, provenance
), dated AS (
    SELECT grouped.*, CASE
        WHEN availability_window = 'SPOT' THEN $6::date
        WHEN right(availability_window, 3) = 'CAL' THEN make_date(left(availability_window,4)::int,1,1)
        WHEN substring(availability_window,6,1) = 'Q' THEN
            make_date(left(availability_window,4)::int,(right(availability_window,1)::int - 1)*3+1,1)
        ELSE make_date(left(availability_window,4)::int,right(availability_window,2)::int,1)
    END AS delivery_date FROM grouped
), ranked AS (
    SELECT dated.*, CASE WHEN delivery_date - $6::date <= 30 THEN 0
                         WHEN delivery_date - $6::date <= 90 THEN 1 ELSE 2 END AS urgency_rank
    FROM dated
), selected AS (
    SELECT * FROM ranked
    ORDER BY urgency_rank, (provenance = 'UNKNOWN'), coalesce(volume_mt,0) DESC,
             market_product COLLATE "C", delivery_point_id, availability_window COLLATE "C",
             provenance COLLATE "C", region COLLATE "C", delivery_point_name COLLATE "C"
    LIMIT $8
)
SELECT guard.exceeded, selected.market_product, selected.delivery_point_id,
       selected.delivery_point_name, selected.region, selected.availability_window,
       selected.provenance, selected.volume_mt::text, selected.max_price_per_mt::text,
       selected.order_count, selected.created_at, selected.urgency_rank
FROM guard LEFT JOIN selected ON NOT guard.exceeded
ORDER BY selected.urgency_rank, (selected.provenance = 'UNKNOWN'), coalesce(selected.volume_mt,0) DESC,
         selected.market_product COLLATE "C", selected.delivery_point_id,
         selected.availability_window COLLATE "C", selected.provenance COLLATE "C",
         selected.region COLLATE "C", selected.delivery_point_name COLLATE "C"
