SELECT id, name, cii_rating, organization_id
FROM vessels
WHERE $1::boolean OR ($2::uuid IS NOT NULL AND organization_id = $2::uuid)
ORDER BY id
LIMIT 2001
