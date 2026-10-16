, requested_keys AS MATERIALIZED (
    SELECT DISTINCT side, product, point, "window"
    FROM jsonb_to_recordset($14::jsonb) AS requested(
        side text, product text, point uuid, "window" text
    )
)
SELECT o.side, o.market_product, o.delivery_point_id, o.availability_window,
       o.remaining_quantity_mt, o.price_per_mt_usd,
       -- Qualification needs declarations, not the complete private order row.
       jsonb_build_object(
           'product_id', o.product_id, 'side', o.side,
           'delivery_point_id', o.delivery_point_id,
           'availability_window', o.availability_window,
           'remaining_quantity_mt', o.remaining_quantity_mt::text,
           'off_spec', o.off_spec, 'msds_available', o.msds_available,
           'certification_scheme', o.certification_scheme,
           'certification_declared', o.certification_declared,
           'fame_terms', o.fame_terms, 'lng_terms', o.lng_terms,
           'carbon_intensity_gco2_mj', o.carbon_intensity_gco2_mj::text,
           'specification_standard', o.specification_standard,
           'feedstock', o.feedstock, 'origin', o.origin
       ) AS private_digest_input
FROM eligible_orders AS o
JOIN requested_keys AS requested
    ON (o.side, o.market_product, o.delivery_point_id, o.availability_window)
        = (requested.side, requested.product, requested.point, requested."window")
