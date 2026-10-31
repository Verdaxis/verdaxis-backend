SELECT id, name, cii_rating, organization_id
FROM vessels
WHERE id = $1::uuid
LIMIT 1
