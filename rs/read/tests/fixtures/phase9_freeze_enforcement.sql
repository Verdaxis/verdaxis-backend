-- Disposable lifecycle observations, installed after the shared trade fixtures.
CREATE SEQUENCE IF NOT EXISTS enforcement_status_attempts;
CREATE OR REPLACE FUNCTION enforcement_status_observe() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM nextval('enforcement_status_attempts');
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS enforcement_status_observe ON trades;
DROP TRIGGER IF EXISTS enforcement_force_frozen ON trades;
CREATE TRIGGER enforcement_status_observe BEFORE UPDATE OF status ON trades
FOR EACH ROW EXECUTE FUNCTION enforcement_status_observe();
GRANT USAGE ON SEQUENCE enforcement_status_attempts TO rust_app;

CREATE OR REPLACE FUNCTION enforcement_hold() RETURNS void LANGUAGE sql AS $$
    SELECT pg_advisory_lock(280029001);
$$;
CREATE OR REPLACE FUNCTION enforcement_release() RETURNS boolean LANGUAGE sql AS $$
    SELECT pg_advisory_unlock(280029001);
$$;
CREATE OR REPLACE FUNCTION enforcement_audit_barrier() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('application_name') = 'enforcement-first' THEN
        PERFORM pg_advisory_xact_lock(280029001);
    END IF;
    RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS enforcement_audit_barrier ON audit_logs;
CREATE TRIGGER enforcement_audit_barrier BEFORE INSERT ON audit_logs
FOR EACH ROW EXECUTE FUNCTION enforcement_audit_barrier();

CREATE OR REPLACE FUNCTION enforcement_raise_frozen() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'synthetic status guard' USING ERRCODE='ZF001';
END $$;
CREATE OR REPLACE FUNCTION enforcement_projection_barrier() RETURNS boolean
LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('application_name') = 'enforcement-read'
       AND current_query() LIKE '%to_jsonb(t)%' THEN
        PERFORM pg_advisory_xact_lock(280029001);
    END IF;
    RETURN true;
END $$;
