-- Authentication-state tables the auth maintenance job reads and writes.
-- Column names and types follow app/models/user.py, refresh_session.py and
-- registration.py; only the columns the job and its races touch are present.
-- The caller sets search_path to a disposable schema first.
CREATE TABLE users(
    id uuid PRIMARY KEY,
    email text NOT NULL,
    role text NOT NULL DEFAULT 'BUYER',
    status text NOT NULL DEFAULT 'APPROVED',
    email_verified boolean NOT NULL DEFAULT true,
    email_verification_token_hash varchar(64),
    email_verification_token_expires_at timestamptz,
    password_reset_token_hash varchar(64),
    password_reset_expires timestamptz,
    pending_approval_email_transition_id uuid,
    pending_approval_email_payload jsonb,
    pending_approval_email_retry_at timestamptz,
    last_login timestamptz
);

CREATE TABLE refresh_sessions(
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id),
    family_id uuid NOT NULL,
    jti_hash varchar(64) NOT NULL UNIQUE,
    device_id_hash varchar(64),
    replaced_by_jti_hash varchar(64),
    rotation_grace_until timestamptz,
    revoked boolean NOT NULL DEFAULT false,
    expires_at timestamptz NOT NULL,
    created_at timestamptz,
    last_used_at timestamptz
);

CREATE TABLE pending_registrations(
    id uuid PRIMARY KEY,
    token_hash varchar(64) NOT NULL UNIQUE,
    email text NOT NULL,
    password_hash text NOT NULL,
    first_name text,
    last_name text,
    role varchar(20),
    referral_code varchar(32),
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz
);
CREATE UNIQUE INDEX uq_pending_registrations_email_active
    ON pending_registrations (lower(email)) WHERE used_at IS NULL;

-- deploy/postgres/app_acl_policy.sql grants the application role table-level
-- DML on all three tables.
GRANT SELECT, INSERT, UPDATE, DELETE ON users, refresh_sessions, pending_registrations TO rust_app;
