SELECT DISTINCT side, product, point, "window"
FROM jsonb_to_recordset($14::jsonb) AS requested(
    side text, product text, point uuid, "window" text
)
