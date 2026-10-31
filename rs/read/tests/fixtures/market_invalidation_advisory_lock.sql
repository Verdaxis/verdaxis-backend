-- Test fixture: session-level advisory lock in the disposable schema.
SELECT pg_advisory_xact_lock($1)
