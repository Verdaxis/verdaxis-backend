-- R02 GET /api/ports/{port_id}: one port with its optional intelligence row.
--
-- Identical projection to `ports.sql`; Python looks the primary key up by its
-- string code and answers 404 when no row matches.
-- Binding: $1 port id (text).
SELECT
    p.id,
    p.name,
    p.country,
    p.timezone,
    p.is_active,
    ST_X(p.location::geometry) AS lng,
    ST_Y(p.location::geometry) AS lat,
    i.port_id AS intelligence_port_id,
    i.congestion_level,
    i.methanol_price_avg::float8 AS methanol_price_avg,
    i.biofuel_price_avg::float8 AS biofuel_price_avg,
    i.captured_at
FROM ports AS p
LEFT JOIN port_intelligence AS i ON i.port_id = p.id
WHERE p.id = $1
