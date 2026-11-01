-- Test fixture for offboarding_command_postgres, applied by the schema owner
-- to a disposable migrated database. It adds two recording tables for the
-- test market hook and one failure switch per table: while a table's name is
-- in offboarding_test_failpoints, any row change on that table raises, so a
-- test can stop the command right after each of its writes.
CREATE TABLE IF NOT EXISTS offboarding_test_hook_calls (
    subject_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    observed_status text NOT NULL,
    observed_marker boolean NOT NULL,
    observed_organization uuid
);
CREATE TABLE IF NOT EXISTS offboarding_test_hook_refusals (subject_id uuid PRIMARY KEY);
CREATE TABLE IF NOT EXISTS offboarding_test_failpoints (table_name text PRIMARY KEY);
GRANT SELECT, INSERT ON offboarding_test_hook_calls TO PUBLIC;
GRANT SELECT ON offboarding_test_hook_refusals TO PUBLIC;

CREATE OR REPLACE FUNCTION offboarding_test_fail() RETURNS trigger
LANGUAGE plpgsql SECURITY DEFINER SET search_path = public AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM offboarding_test_failpoints WHERE table_name = TG_TABLE_NAME) THEN
        RAISE EXCEPTION 'injected failure after a change to %', TG_TABLE_NAME USING ERRCODE = 'P0001';
    END IF;
    RETURN NULL;
END $$;

DO $$
DECLARE
    target text;
BEGIN
    FOREACH target IN ARRAY ARRAY[
        'users', 'audit_logs', 'user_status_transitions', 'refresh_sessions',
        'password_reset_tokens', 'organization_join_requests', 'market_support_contexts',
        'market_support_authorizations', 'staff_capability_assignments',
        'market_event_outbox', 'individual_offboardings'
    ] LOOP
        EXECUTE format(
            'CREATE OR REPLACE TRIGGER offboarding_test_fail AFTER INSERT OR UPDATE OR DELETE '
            'ON %I FOR EACH ROW EXECUTE FUNCTION offboarding_test_fail()', target);
    END LOOP;
END $$;
