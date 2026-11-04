-- Installed by the database administrator in each cloned database of the
-- integrated offboarding acceptance target, after
-- fixtures/offboarding_market.sql and fixtures/offboarding_composition.sql.
-- Fixture writes stay here, outside the Rust sources the order-book write
-- boundary scans.

-- A gate that one test session closes and opens. While it is armed, a gated
-- write waits for it inside its own transaction, after that transaction has
-- taken its earlier row locks. Opening disarms it first, so a write that
-- reaches the trigger later passes at once. The function's own lock bound
-- replaces the caller's short one only while it waits.
CREATE TABLE IF NOT EXISTS acceptance_gate_state (only_row boolean PRIMARY KEY DEFAULT true CHECK (only_row), armed boolean NOT NULL);
INSERT INTO acceptance_gate_state(armed) VALUES (false) ON CONFLICT DO NOTHING;
GRANT SELECT ON acceptance_gate_state TO PUBLIC;

CREATE OR REPLACE FUNCTION acceptance_gate_close() RETURNS void LANGUAGE sql AS $$
    UPDATE acceptance_gate_state SET armed = true;
    SELECT pg_advisory_lock(727700901);
$$;

-- Run in the session that closed the gate, in autocommit: the disarm is
-- committed before the waiting writer takes the lock.
CREATE OR REPLACE FUNCTION acceptance_gate_open() RETURNS boolean LANGUAGE sql AS $$
    UPDATE acceptance_gate_state SET armed = false;
    SELECT pg_advisory_unlock(727700901);
$$;

CREATE OR REPLACE FUNCTION acceptance_gate_wait() RETURNS trigger
LANGUAGE plpgsql SET lock_timeout = '20s' AS $$
BEGIN
    IF (SELECT armed FROM acceptance_gate_state) THEN
        PERFORM pg_advisory_xact_lock_shared(727700901);
    END IF;
    RETURN NEW;
END $$;

-- One gate trigger on the write that a request makes after its row locks.
CREATE OR REPLACE FUNCTION acceptance_gate_install(target text) RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    PERFORM acceptance_gate_remove();
    IF target = 'session-insert' THEN
        CREATE TRIGGER acceptance_gate BEFORE INSERT ON refresh_sessions
            FOR EACH ROW EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'session-revoke' THEN
        -- After the row change, so the waiting writer holds the row lock.
        CREATE TRIGGER acceptance_gate AFTER UPDATE OF revoked ON refresh_sessions
            FOR EACH ROW WHEN (NOT OLD.revoked AND NEW.revoked) EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'status-transition' THEN
        CREATE TRIGGER acceptance_gate BEFORE INSERT ON user_status_transitions
            FOR EACH ROW EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'join-approval' THEN
        CREATE TRIGGER acceptance_gate BEFORE UPDATE OF status ON organization_join_requests
            FOR EACH ROW WHEN (NEW.status = 'APPROVED') EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'reset-secret' THEN
        CREATE TRIGGER acceptance_gate BEFORE UPDATE OF password_reset_token_hash ON users
            FOR EACH ROW WHEN (NEW.password_reset_token_hash IS NOT NULL) EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'password-set' THEN
        CREATE TRIGGER acceptance_gate BEFORE UPDATE OF password_hash ON users
            FOR EACH ROW WHEN (NEW.password_reset_token_hash IS NULL AND OLD.password_reset_token_hash IS NOT NULL)
            EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'trade-reassignment' THEN
        CREATE TRIGGER acceptance_gate BEFORE UPDATE OF buyer_user_id, seller_user_id ON trades
            FOR EACH ROW WHEN (OLD.buyer_user_id IS DISTINCT FROM NEW.buyer_user_id
                OR OLD.seller_user_id IS DISTINCT FROM NEW.seller_user_id)
            EXECUTE FUNCTION acceptance_gate_wait();
    ELSIF target = 'trade-settlement' THEN
        CREATE TRIGGER acceptance_gate BEFORE UPDATE OF status ON trades
            FOR EACH ROW WHEN (NEW.status = 'DELIVERED') EXECUTE FUNCTION acceptance_gate_wait();
    ELSE
        RAISE EXCEPTION 'unknown acceptance gate %', target;
    END IF;
END $$;

CREATE OR REPLACE FUNCTION acceptance_gate_remove() RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    DROP TRIGGER IF EXISTS acceptance_gate ON refresh_sessions;
    DROP TRIGGER IF EXISTS acceptance_gate ON user_status_transitions;
    DROP TRIGGER IF EXISTS acceptance_gate ON organization_join_requests;
    DROP TRIGGER IF EXISTS acceptance_gate ON users;
    DROP TRIGGER IF EXISTS acceptance_gate ON trades;
END $$;

-- A known password for a fixture person (the hash is computed by the test).
CREATE OR REPLACE FUNCTION acceptance_password(member uuid, hash text) RETURNS void LANGUAGE sql AS $$
    UPDATE users SET password_hash = hash WHERE id = member;
$$;

-- A pending request of a person to join an organization.
CREATE OR REPLACE FUNCTION acceptance_join_request(request uuid, member uuid, organization uuid) RETURNS void LANGUAGE sql AS $$
    INSERT INTO organization_join_requests(id,user_id,organization_id,status,created_at)
        VALUES(request,member,organization,'PENDING',now());
$$;

-- A reset secret as the Python forgot-password route stores it: the SHA-256
-- hex digest of the mailed token, valid for one hour.
CREATE OR REPLACE FUNCTION acceptance_reset_secret(member uuid, digest text) RETURNS void LANGUAGE sql AS $$
    UPDATE users SET password_reset_token_hash = digest, password_reset_expires = now() + interval '1 hour'
        WHERE id = member;
$$;

-- The reviewed manual step of a reinstatement: the operator clears the marker
-- and nothing else. The offboarding record, both cutoffs and the cleared
-- credentials stay as they are.
CREATE OR REPLACE FUNCTION acceptance_reviewed_marker_clear(member uuid) RETURNS void LANGUAGE sql AS $$
    UPDATE users SET offboarded_at = NULL WHERE id = member;
$$;

-- The account fields an offboarding or a reinstatement can change.
CREATE OR REPLACE FUNCTION acceptance_account(member uuid) RETURNS jsonb LANGUAGE sql AS $$
    SELECT jsonb_build_object(
        'status', u.status, 'organization_id', u.organization_id, 'offboarded_at', u.offboarded_at,
        'password_hash', u.password_hash, 'password_changed_at', u.password_changed_at,
        'authentication_revoked_at', u.authentication_revoked_at,
        'password_reset_token_hash', u.password_reset_token_hash,
        'kyc_status', u.kyc_status, 'kyc_organization_id', u.kyc_organization_id,
        'email_verified', u.email_verified, 'must_change_password', u.must_change_password,
        'live_sessions', (SELECT count(*) FROM refresh_sessions s WHERE s.user_id = u.id AND NOT s.revoked),
        'sessions', (SELECT count(*) FROM refresh_sessions s WHERE s.user_id = u.id),
        'reset_tokens', (SELECT count(*) FROM password_reset_tokens r WHERE r.user_id = u.id),
        'joins', (SELECT coalesce(jsonb_object_agg(j.id, j.status), '{}') FROM organization_join_requests j WHERE j.user_id = u.id),
        'records', (SELECT count(*) FROM individual_offboardings o WHERE o.subject_user_id = u.id),
        'rows_with_email', (SELECT count(*) FROM users other WHERE lower(other.email) = lower(u.email)))
    FROM users u WHERE u.id = member;
$$;
