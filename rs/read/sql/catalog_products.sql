-- R03 GET /api/catalog/products (Task 8).
--
-- Python keeps canonical product identity in code (`CANONICAL_PRODUCTS`) and
-- filters the database rows with `canonical_product_clause`, then orders by the
-- canonical display position and the row name. The policy artifact carries that
-- same identity, lane, and order as data, so this statement joins it instead of
-- naming a product, a product code, or a product UUID.
--
-- The mutable columns stay in the database: unit, minimum lot, and description.
-- `min_lot_size` is projected as text at its stored scale, which is exactly the
-- decimal string the Python response model emits.
-- Binding: $1 attested market policy (jsonb text).
WITH policy_products AS (
    SELECT *
    FROM jsonb_to_recordset(($1::jsonb) -> 'products') AS policy_product(
        id uuid,
        code text,
        name text,
        fuel_type text,
        fuel_grade text,
        execution_mode text,
        lanes jsonb,
        qualification_profile text,
        specification_standard text,
        display_order integer
    )
)
SELECT
    p.id::text AS id,
    p.name,
    p.fuel_type,
    p.fuel_grade,
    p.unit,
    p.min_lot_size::text AS min_lot_size,
    p.spec_description,
    p.is_active
FROM products AS p
JOIN policy_products AS pp
    ON pp.id = p.id
    AND pp.name = p.name
    AND pp.fuel_type = p.fuel_type
    AND pp.fuel_grade = p.fuel_grade
WHERE p.is_active
ORDER BY pp.display_order, p.name
