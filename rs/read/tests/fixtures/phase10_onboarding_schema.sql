-- Minimal shapes of the tables the onboarding attention monitor reads.
-- Column names and meanings follow the application models; only the columns
-- the candidate projection touches are present.
CREATE TABLE organizations (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    verification_status text NOT NULL,
    provenance text NOT NULL
);
CREATE TABLE users (
    id uuid PRIMARY KEY,
    email text NOT NULL,
    first_name text,
    last_name text,
    role text,
    status text NOT NULL,
    organization_id uuid REFERENCES organizations (id),
    created_at timestamptz NOT NULL,
    email_verified boolean NOT NULL DEFAULT false,
    email_verification_token_expires_at timestamptz,
    last_login timestamptz
);
CREATE TABLE organization_join_requests (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id),
    organization_id uuid NOT NULL REFERENCES organizations (id),
    status text NOT NULL,
    reviewed_at timestamptz,
    created_at timestamptz,
    UNIQUE (user_id, organization_id)
);
CREATE TABLE user_status_transitions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id),
    to_status text NOT NULL,
    effective_at timestamptz NOT NULL
);
CREATE TABLE audit_logs (
    id uuid PRIMARY KEY,
    action varchar(100),
    resource_id varchar(100),
    "timestamp" timestamptz
);
CREATE TABLE pending_registrations (
    id uuid PRIMARY KEY,
    email text NOT NULL,
    first_name text,
    last_name text,
    role text,
    expires_at timestamptz NOT NULL,
    used_at timestamptz,
    created_at timestamptz NOT NULL
);
