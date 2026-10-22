, requested_ids AS (
    SELECT DISTINCT unnest($14::uuid[]) AS id
), fleet_count AS (
    SELECT count(*) AS vessel_count
    FROM vessels
    WHERE $16::boolean OR (
        $15::uuid IS NOT NULL AND organization_id = $15::uuid
    )
)
SELECT requested_ids.id AS requested_id,
       eligible_orders.product_fuel_type AS fuel_type,
       eligible_orders.carbon_intensity_gco2_mj AS ci,
       eligible_orders.energy_density_mj_kg AS lcv,
       fleet_count.vessel_count AS fleet_vessel_count
FROM fleet_count
LEFT JOIN requested_ids ON true
LEFT JOIN eligible_orders ON eligible_orders.id = requested_ids.id
                           AND eligible_orders.side::text = 'ASK'
ORDER BY requested_ids.id
