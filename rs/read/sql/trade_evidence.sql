-- Economics accepts only exact REAL/REAL or DEMO/DEMO pairs.
--
-- A flat tail over `formal_trades` rather than a nested `SELECT *` wrapper: a
-- wrapper level shows up as a Subquery Scan that stops the planner pushing a
-- consumer's aggregation into the parallel workers (measured on the reference
-- aggregate: 87.5k rows pulled into one serial sort, 671-936 ms, where the
-- same aggregation inside the workers ran in 289-491 ms). Reading the CTE
-- directly keeps one predicate definition and lets the consumer's GROUP BY
-- push below the snapshot predicates.
SELECT * FROM formal_trades
WHERE (buyer_provenance = 'REAL' AND seller_provenance = 'REAL')
   OR (buyer_provenance = 'DEMO' AND seller_provenance = 'DEMO')
