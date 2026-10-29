SELECT id, name, fuel_type, capacity_kt_per_year::text,
       country, region,
       ST_Y(location::geometry) AS lat,
       ST_X(location::geometry) AS lng,
       cod_date, cod_year, status, data_source, gena_project_id,
       organization_id, feedstock, technology,
       carbon_intensity_gco2_mj::text, notes, created_at
FROM producer_projects
WHERE ($1::text IS NULL OR fuel_type ILIKE '%' || $1 || '%')
  AND ($2::text IS NULL OR country ILIKE '%' || $2 || '%')
  AND ($3::text IS NULL OR status = $3)
  AND ($4::integer IS NULL OR cod_year >= $4)
  AND ($5::integer IS NULL OR cod_year <= $5)
ORDER BY cod_year ASC NULLS LAST, name ASC, id ASC
LIMIT $6
