-- A write that the inspection transaction must refuse. The probe lives in a
-- function so no Rust source names a write statement. Load as the database
-- owner. The function is SECURITY INVOKER: it succeeds in an ordinary
-- transaction run by the owner and fails with SQLSTATE 25006 inside a
-- read-only one.
CREATE TABLE inspect_write_probe (written_at timestamptz NOT NULL DEFAULT now());
CREATE FUNCTION inspect_write_probe_run() RETURNS integer
LANGUAGE sql
AS $$
  WITH inserted AS (INSERT INTO inspect_write_probe DEFAULT VALUES RETURNING 1)
  SELECT count(*)::integer FROM inserted
$$;
