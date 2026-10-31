-- Test fixture: pause an organization write inside its market stage.
--
-- The trigger makes the first outbox insert of a transaction wait for a
-- transaction-scoped advisory lock that the test holds on another connection,
-- so the write is observed (as blocked) after it took its organization, user,
-- slice and order locks and wrote the stock release, and before it commits.
CREATE FUNCTION race_stage_gate() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    -- The market stage set a 500 ms local lock timeout; the test's own wait is not
    -- part of the behaviour under test, so it is exempt from that bound.
    PERFORM set_config('lock_timeout', '0', true);
    PERFORM pg_advisory_xact_lock(7240001);
    RETURN NEW;
END
$$;
CREATE TRIGGER race_stage_gate BEFORE INSERT ON market_event_outbox
    FOR EACH ROW EXECUTE FUNCTION race_stage_gate();
