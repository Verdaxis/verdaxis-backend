-- Phase 0 Task 6: standardize canonical database sessions on UTC.
--
-- Apply once per target database from a connection to `postgres`, as the
-- database owner or a superuser. Required psql variables, with literal
-- allowlisted names chosen by the calling tool:
--
--   database_name, app_role, migrator_role, backup_role
--
-- Example:
--   psql -X -d postgres -v ON_ERROR_STOP=1 \
--     -v database_name=verdaxis_prod -v app_role=verdaxis_app \
--     -v migrator_role=verdaxis_migrator -v backup_role=verdaxis_backup \
--     -f deploy/phase0/set_timezone.sql
--
-- The two statement kinds together beat every server-side source:
--   * `ALTER DATABASE` sets the default for the database;
--   * `ALTER ROLE ... IN DATABASE` is the highest server-side priority and
--     overrides both a database default and a role-wide setting.
-- A connection option such as PGOPTIONS or a startup `timezone` parameter
-- outranks all of them and is invisible here. The operator and the tooling must
-- inspect and reject unexplained connection options before trusting
-- `SHOW TimeZone`.
--
-- A database default does not change an already-open session. Recycle the API
-- connection pools and every CLI process after this runs.

\set ON_ERROR_STOP on

-- The policy runs from the maintenance database, never from the target.
SELECT 1 / (current_database() = 'postgres')::integer AS connected_to_postgres;

-- The retired production database is never a target.
SELECT 1 / (:'database_name' <> 'verdaxis')::integer AS target_is_not_retired;

SELECT 1 / (
    :'app_role' <> :'migrator_role'
    AND :'app_role' <> :'backup_role'
    AND :'migrator_role' <> :'backup_role'
)::integer AS roles_are_distinct;

-- Refuse an unexplained role-wide timezone override. A role-wide setting
-- (setdatabase = 0) applies to every database that role connects to, including
-- the retired one, and this statement set does not manage it. The target
-- database's own existing default is replaced below, so it is not refused.
SELECT 1 / (count(*) = 0)::integer AS no_unexplained_role_wide_timezone
FROM pg_catalog.pg_db_role_setting AS setting
WHERE setting.setrole IN (
        SELECT oid FROM pg_catalog.pg_roles
        WHERE rolname IN (:'app_role', :'migrator_role', :'backup_role')
    )
  AND setting.setdatabase = 0
  AND EXISTS (
        SELECT 1 FROM unnest(setting.setconfig) AS option
        WHERE lower(split_part(option, '=', 1)) = 'timezone'
    );

ALTER DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"app_role" IN DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"migrator_role" IN DATABASE :"database_name" SET timezone TO 'UTC';
ALTER ROLE :"backup_role" IN DATABASE :"database_name" SET timezone TO 'UTC';

-- Require exactly the four UTC settings this policy publishes. A leftover
-- non-UTC entry on the target fails the run instead of passing silently.
SELECT 1 / ((
    SELECT count(*) FROM pg_catalog.pg_db_role_setting AS setting
    WHERE setting.setdatabase = (
            SELECT oid FROM pg_catalog.pg_database WHERE datname = :'database_name'
        )
      AND (
            setting.setrole = 0
            OR setting.setrole IN (
                SELECT oid FROM pg_catalog.pg_roles
                WHERE rolname IN (:'app_role', :'migrator_role', :'backup_role')
            )
        )
      AND EXISTS (
            SELECT 1 FROM unnest(setting.setconfig) AS option
            WHERE lower(split_part(option, '=', 1)) = 'timezone'
              AND lower(split_part(option, '=', 2)) = 'utc'
        )
      AND NOT EXISTS (
            SELECT 1 FROM unnest(setting.setconfig) AS option
            WHERE lower(split_part(option, '=', 1)) = 'timezone'
              AND lower(split_part(option, '=', 2)) <> 'utc'
        )
) = 4)::integer AS published_four_utc_settings;
