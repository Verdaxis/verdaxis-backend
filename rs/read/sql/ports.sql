-- R01 GET /api/ports: every port with its optional intelligence row.
--
-- The Python route selects all ports, extracts the coordinates from the
-- geography column, and applies neither a filter nor an ORDER BY, so this
-- statement does the same: row order is whatever PostgreSQL returns.
--
-- `location` is deliberately not selected. Pydantic cannot serialize the
-- geography value, so the route returns an explicit null in its place.
-- Binding: none.
SELECT
    p.id,
    p.name,
    p.country,
    p.timezone,
    p.is_active,
    ST_X(p.location::geometry) AS lng,
    ST_Y(p.location::geometry) AS lat,
    -- The join key is the presence marker: a null value means the port has no
    -- intelligence row, which Python models as `intelligence: null`.
    i.port_id AS intelligence_port_id,
    i.congestion_level,
    i.methanol_price_avg::float8 AS methanol_price_avg,
    i.biofuel_price_avg::float8 AS biofuel_price_avg,
    i.captured_at
FROM ports AS p
LEFT JOIN port_intelligence AS i ON i.port_id = p.id
