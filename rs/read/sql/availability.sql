-- One statement snapshot; $1 is the attested policy, $2 the optional filter,
-- and $3 the fixed result cap plus one sentinel group.
WITH policy_products AS MATERIALIZED (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'products') AS p(
        id uuid, code text, name text, fuel_type text, fuel_grade text,
        execution_mode text, qualification_profile text,
        specification_standard text, lanes jsonb
    )
), policy_points AS MATERIALIZED (
    SELECT * FROM jsonb_to_recordset(($1::jsonb)->'points') AS p(
        id uuid, name text, region text
    )
), inventory_evidence AS (
    SELECT port.id AS port_id, port.name AS port_name, port.location,
           pp.code AS market_product, p.id AS product_id,
           i.supplier_id, i.current_stock_mt, i.price_per_mt_usd,
           CASE WHEN org.verification_status = 'APPROVED'
                     AND u.status = 'APPROVED' AND u.kyc_status = 'APPROVED'
                THEN CASE
                    WHEN org.provenance = 'REAL' THEN 'REAL'
                    WHEN org.provenance = 'DEMO' AND org.id IN (
                        '4da7b285-34ee-5443-9406-f96b4ed1a251'::uuid,
                        '0dbce576-2026-5925-ab66-674d505e98ad'::uuid,
                        '3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a'::uuid,
                        '277491df-cb0d-5f2d-a2cf-5746829c6da6'::uuid,
                        '3b302066-d65c-5c3e-8fcc-70b3da3bcafd'::uuid,
                        '79609f48-0a3e-560e-a1e1-63d90601d84a'::uuid,
                        '2c4e387e-de22-5adb-ad88-9274ba84ebe1'::uuid,
                        '612953c7-567a-58b3-bc42-ee817d2bbe74'::uuid,
                        '93ccda09-54b3-53ee-afc0-759d3048161f'::uuid,
                        '82426590-0963-5486-9b05-f81e97afe6ef'::uuid,
                        'acc3f20a-fe94-4463-9029-a55e35634eb7'::uuid,
                        'c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4'::uuid,
                        '7cc77115-0a9f-4ec4-8c74-05aa10050111'::uuid,
                        'd1e43e55-3fb0-4b5e-9f0b-93aa10050222'::uuid
                    ) THEN 'DEMO'
                    ELSE 'UNKNOWN'
                END ELSE 'UNKNOWN'
           END AS evidence
    FROM inventory_items i
    JOIN ports port ON port.id = i.port_id AND port.is_active
    JOIN products p ON p.name = i.product_name AND p.is_active
    JOIN policy_products pp ON pp.id = p.id AND pp.name = p.name
        AND pp.fuel_type = p.fuel_type AND pp.fuel_grade = p.fuel_grade
        AND pp.execution_mode = 'ORDERBOOK'
        AND pp.qualification_profile <> 'lng_v1'
    JOIN organizations org ON org.id = i.supplier_id
        AND org.provenance NOT IN ('TEST', 'CANARY')
    LEFT JOIN users u ON u.id = i.owner_user_id AND u.organization_id = org.id
    WHERE EXISTS (
        SELECT 1 FROM delivery_points dp
        JOIN policy_points pq ON pq.id = dp.id AND pq.name = dp.name
            AND pq.region = dp.region
        WHERE dp.name = port.name AND dp.is_active
          AND pp.lanes ? dp.id::text
    )
      AND ($2::text IS NULL OR $2 = '' OR p.name ILIKE '%' || $2 || '%'
           OR p.fuel_type ILIKE '%' || $2 || '%' OR p.fuel_grade ILIKE '%' || $2 || '%')
      AND (pp.specification_standard IS NULL OR (
          NOT i.off_spec
          AND upper(btrim(i.specification_standard, E' \t\r\n\f\013')) = pp.specification_standard
          AND length(btrim(i.carbon_intensity_method, E' \t\r\n\f\013')) > 0
          AND i.certification_declared
          AND length(btrim(i.certification_scheme, E' \t\r\n\f\013')) > 0
          AND i.msds_available AND i.carbon_intensity_gco2_mj IS NOT NULL
          AND length(btrim(i.feedstock, E' \t\r\n\f\013')) > 0
          AND length(btrim(i.origin, E' \t\r\n\f\013')) > 0
      ))
), grouped AS (
    SELECT port_id, port_name, product_id, market_product, location,
           sum(current_stock_mt) FILTER (WHERE evidence = 'REAL') AS real_stock,
           sum(current_stock_mt) FILTER (WHERE evidence = 'DEMO') AS demo_stock,
           count(DISTINCT supplier_id) FILTER (WHERE evidence = 'REAL') AS real_count,
           count(DISTINCT supplier_id) FILTER (WHERE evidence = 'DEMO') AS demo_count,
           count(DISTINCT supplier_id) FILTER (WHERE evidence = 'UNKNOWN') AS unknown_count,
           avg(price_per_mt_usd) FILTER (WHERE evidence = 'REAL') AS real_price,
           avg(price_per_mt_usd) FILTER (WHERE evidence = 'DEMO') AS demo_price
    FROM inventory_evidence
    GROUP BY port_id, port_name, product_id, market_product, location
    ORDER BY port_id, product_id
    LIMIT $3
), selected AS (
    SELECT *,
           CASE WHEN real_count > 0 THEN coalesce(real_stock, 0)
                WHEN demo_count > 0 THEN coalesce(demo_stock, 0) END AS stock,
           CASE WHEN real_count > 0 THEN real_price
                WHEN demo_count > 0 THEN demo_price END AS price,
           CASE WHEN real_count > 0 THEN real_count
                WHEN demo_count > 0 THEN demo_count ELSE 0 END AS supplier_count,
           CASE WHEN real_count > 0 THEN 'LIVE_INVENTORY'
                WHEN demo_count > 0 THEN 'DEMO_SEED'
                WHEN unknown_count > 0 THEN 'UNKNOWN' ELSE 'NO_DATA' END AS source_kind,
           CASE WHEN real_count > 0 THEN 'REAL_ONLY'
                WHEN demo_count > 0 THEN 'DEMO_ONLY'
                WHEN unknown_count > 0 THEN 'UNKNOWN' ELSE 'NOT_APPLICABLE' END AS demo_status
    FROM grouped
)
SELECT port_id, port_name, market_product,
       ST_Y(location::geometry) AS lat, ST_X(location::geometry) AS lng,
       stock::text AS total_stock_mt, round(price, 2)::text AS avg_price_per_mt,
       supplier_count, unknown_count, source_kind, demo_status,
       CASE WHEN stock > 1000 THEN 'AVAILABLE' WHEN stock > 0 THEN 'LIMITED'
            ELSE 'NONE' END AS availability_level
FROM selected
ORDER BY port_id, product_id
