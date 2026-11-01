-- Analytics retention fixture (test only).
--
-- Production table names, column shapes, keys and indexes of the two Product
-- Analytics fact tables, plus seeding and failure-injection functions. Loaded
-- into a throwaway schema of a disposable database; never applied to a
-- deployed database.

CREATE TABLE users (
    id uuid PRIMARY KEY
);

CREATE TABLE user_login_days (
    id uuid PRIMARY KEY,
    activity_date date NOT NULL,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    organization_id uuid,
    role varchar(8),
    login_count integer NOT NULL,
    first_login_at timestamptz NOT NULL,
    last_login_at timestamptz NOT NULL,
    CONSTRAINT uq_user_login_days_date_user UNIQUE (activity_date, user_id)
);
CREATE INDEX ix_user_login_days_date_role ON user_login_days (activity_date, role);
CREATE INDEX ix_user_login_days_org_date ON user_login_days (organization_id, activity_date);

CREATE TABLE user_status_transitions (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    organization_id uuid,
    role varchar(8),
    from_status varchar(8),
    to_status varchar(8) NOT NULL,
    effective_at timestamptz NOT NULL,
    provenance varchar(32) NOT NULL
);
CREATE INDEX ix_user_status_transitions_user_time ON user_status_transitions (user_id, effective_at);
CREATE INDEX ix_user_status_transitions_status_time ON user_status_transitions (to_status, effective_at);

-- One user per call index, so a seed is repeatable and counts are exact.
CREATE FUNCTION seed_users(user_count integer) RETURNS void
LANGUAGE sql AS $$
    INSERT INTO users (id) SELECT gen_random_uuid() FROM generate_series(1, user_count);
$$;

-- One login-day row per user for every date from first_day to last_day.
CREATE FUNCTION seed_login_days(first_day date, last_day date) RETURNS bigint
LANGUAGE sql AS $$
    WITH inserted AS (
        INSERT INTO user_login_days
            (id, activity_date, user_id, role, login_count, first_login_at, last_login_at)
        SELECT gen_random_uuid(), day::date, users.id, 'BUYER', 1,
               day + interval '8 hours', day + interval '8 hours'
        FROM generate_series(first_day::timestamp, last_day::timestamp, interval '1 day') AS day
        CROSS JOIN users
        RETURNING 1
    )
    SELECT count(*) FROM inserted;
$$;

-- One transition per user, dated far outside any login-day retention window.
CREATE FUNCTION seed_status_transitions(effective timestamptz) RETURNS bigint
LANGUAGE sql AS $$
    WITH inserted AS (
        INSERT INTO user_status_transitions
            (id, user_id, role, from_status, to_status, effective_at, provenance)
        SELECT gen_random_uuid(), users.id, 'BUYER', 'PENDING', 'APPROVED', effective, 'workflow'
        FROM users
        RETURNING 1
    )
    SELECT count(*) FROM inserted;
$$;

CREATE SEQUENCE prune_failure_attempts;

-- Statement trigger for the retry cases. It raises the given SQLSTATE while the
-- attempt number is at most the first argument, then lets the delete through.
-- The sequence is not transactional, so a rolled-back attempt still counts.
CREATE FUNCTION fail_first_attempts() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF nextval('prune_failure_attempts') <= TG_ARGV[0]::bigint THEN
        RAISE EXCEPTION 'injected failure' USING ERRCODE = TG_ARGV[1];
    END IF;
    RETURN NULL;
END;
$$;
