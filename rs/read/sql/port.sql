-- Port rows only; intelligence is loaded in select-in batches after this query.
-- No active filter or ORDER BY: preserve the Python port selection.
SELECT
    p.id,
    p.name,
    p.country,
    p.timezone,
    p.is_active,
    ST_X(p.location::geometry) AS lng,
    ST_Y(p.location::geometry) AS lat
FROM ports AS p
WHERE p.id = $1
