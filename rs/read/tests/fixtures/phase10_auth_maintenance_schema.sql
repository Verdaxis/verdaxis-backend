-- Authentication-state tables the auth maintenance job reads and writes.
-- Column names and types follow app/models/user.py, refresh_session.py,
-- registration.py and the reset-token migration; only the columns the job
-- and its races touch are present.
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

-- Rust-owned reset tokens (alembic/versions/p3_20261020_auth_tokens.py).
CREATE TABLE password_reset_tokens(
    user_id uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    token_hash text NOT NULL UNIQUE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    used_at timestamptz
);
CREATE INDEX ix_password_reset_tokens_expires_at ON password_reset_tokens (expires_at);

-- deploy/postgres/app_acl_policy.sql: table-level DML on the first three
-- tables; SELECT, DELETE and per-column INSERT/UPDATE on the reset tokens
-- (row locks need UPDATE on at least one column).
GRANT SELECT, INSERT, UPDATE, DELETE ON users, refresh_sessions, pending_registrations TO rust_app;
GRANT SELECT, DELETE ON password_reset_tokens TO rust_app;
GRANT INSERT (user_id, token_hash, expires_at, created_at, used_at),
      UPDATE (token_hash, expires_at, created_at, used_at)
    ON password_reset_tokens TO rust_app;
