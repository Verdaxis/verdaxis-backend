-- Rust decision 24: the one shared definition of the private digest input.
--
-- The shared visibility relation no longer carries this object. A consumer that
-- keeps rows calls this fragment for exactly those rows, after its own filter,
-- sort and limit, as Python does. `/*row_alias*/` is the order-row alias in the
-- calling statement and must name the base `orderbook_orders` row.
--
-- Internal only. This object is private; never serialize it as a public DTO.
to_jsonb(/*row_alias*/) || jsonb_build_object(
    'quantity_mt', /*row_alias*/.quantity_mt::text,
    'remaining_quantity_mt', /*row_alias*/.remaining_quantity_mt::text,
    'price_per_mt_usd', /*row_alias*/.price_per_mt_usd::text,
    'carbon_intensity_gco2_mj', /*row_alias*/.carbon_intensity_gco2_mj::text,
    'energy_density_mj_kg', /*row_alias*/.energy_density_mj_kg::text
) AS private_digest_input
