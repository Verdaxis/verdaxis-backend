-- R04 GET /api/catalog/delivery-points (Task 8).
--
-- The delivery-point counterpart of `catalog_products.sql`: strict canonical
-- identity from the policy artifact, active rows only, canonical display order
-- then name. `timezone` is nullable in the database and stays null when it is
-- absent, exactly as the Python response model writes it.
-- Binding: $1 attested market policy (jsonb text).
WITH policy_points AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'points') AS policy_point(
        id uuid,
        name text,
        region text,
        timezone text,
        display_order integer
    )
)
SELECT
    dp.id::text AS id,
    dp.name,
    dp.region,
    dp.timezone,
    dp.is_active
FROM delivery_points AS dp
JOIN policy_points AS pp
    ON pp.id = dp.id
    AND pp.name = dp.name
    AND pp.region = dp.region
WHERE dp.is_active
ORDER BY pp.display_order, dp.name
