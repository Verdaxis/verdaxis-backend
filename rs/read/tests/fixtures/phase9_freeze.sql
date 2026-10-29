-- Disposable route barrier: the first request holds its normal rows before audit.
CREATE OR REPLACE FUNCTION freeze_fixture_hold() RETURNS void LANGUAGE sql AS $$
    SELECT pg_advisory_lock(260029001);
$$;
CREATE OR REPLACE FUNCTION freeze_fixture_release() RETURNS boolean LANGUAGE sql AS $$
    SELECT pg_advisory_unlock(260029001);
$$;
CREATE OR REPLACE FUNCTION freeze_fixture_audit_barrier() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('application_name') = 'freeze-route-first' THEN
        PERFORM pg_advisory_xact_lock(260029001);
    END IF;
    RETURN NEW;
END $$;
CREATE TRIGGER freeze_fixture_audit_barrier BEFORE INSERT ON audit_logs
FOR EACH ROW EXECUTE FUNCTION freeze_fixture_audit_barrier();
