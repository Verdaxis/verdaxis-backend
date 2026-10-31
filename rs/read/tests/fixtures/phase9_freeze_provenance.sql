-- Installed only by the disposable freeze target. Trade provenance is
-- immutable under ordinary triggers, so the change runs with them off.
CREATE OR REPLACE FUNCTION freeze_fixture_set_provenance(trade uuid, buyer text, seller text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('session_replication_role','replica',true);
    UPDATE trades SET buyer_provenance=buyer,seller_provenance=seller WHERE id=trade;
    IF NOT FOUND THEN RAISE EXCEPTION 'unknown fixture trade'; END IF;
    PERFORM set_config('session_replication_role','origin',true);
END $$;
