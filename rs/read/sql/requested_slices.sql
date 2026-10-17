-- Rust decision 27: the requested benchmark keys with each market product code
-- resolved to its catalog product id, so the base order scan can filter on the
-- full row-value key set before the shared eligibility predicate.
--
-- The code-to-id mapping reads the same attested policy the relation reads
-- (`$1`), so it cannot drift from `market_product`. Keying on the product id
-- (not the code) lets the scan use the existing orderbook indexes; no index and
-- no schema change is made. Requires `requested_keys`.
SELECT DISTINCT k.side, catalog.id AS product_id, k.point, k."window"
FROM requested_keys AS k
JOIN jsonb_to_recordset(($1::jsonb) -> 'products') AS catalog(id uuid, code text)
  ON catalog.code = k.product
