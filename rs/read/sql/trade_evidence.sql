-- Economics accepts only exact REAL/REAL or DEMO/DEMO pairs.
SELECT * FROM formal_trades
WHERE (buyer_provenance = 'REAL' AND seller_provenance = 'REAL')
   OR (buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO')
