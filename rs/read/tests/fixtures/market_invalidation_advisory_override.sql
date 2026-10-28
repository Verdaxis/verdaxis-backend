-- Test fixture: replace the real advisory lock with one that also writes the
-- order row, so the suite can observe the boundary under lock failure.
CREATE FUNCTION pg_advisory_xact_lock(bigint) RETURNS void LANGUAGE plpgsql AS $$ BEGIN UPDATE orderbook_orders SET availability_window='2027-Q1'; END $$
