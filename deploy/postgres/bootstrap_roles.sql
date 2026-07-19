\set ON_ERROR_STOP on

-- Required psql variables: database_name, app_role, migrator_role, backup_role.
-- Passwords are deliberately absent; provision them through the secret manager.
SELECT 1 / (current_database() = :'database_name')::integer;

BEGIN;

SELECT format(
    'CREATE ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT',
    role_name
)
FROM (VALUES (:'app_role'), (:'migrator_role'), (:'backup_role')) AS roles(role_name)
WHERE NOT EXISTS (SELECT 1 FROM pg_catalog.pg_roles WHERE rolname = role_name)
\gexec

SELECT format(
    'ALTER ROLE %I LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOINHERIT NOBYPASSRLS',
    role_name
)
FROM (VALUES (:'app_role'), (:'migrator_role'), (:'backup_role')) AS roles(role_name)
\gexec

REVOKE CONNECT, CREATE, TEMPORARY ON DATABASE :"database_name" FROM PUBLIC;
GRANT CONNECT ON DATABASE :"database_name" TO :"app_role", :"migrator_role", :"backup_role";
GRANT CREATE, TEMPORARY ON DATABASE :"database_name" TO :"migrator_role";
REVOKE ALL ON SCHEMA public FROM PUBLIC;
ALTER SCHEMA public OWNER TO :"migrator_role";
GRANT USAGE ON SCHEMA public TO :"app_role", :"backup_role";

SELECT format('ALTER TABLE %I.%I OWNER TO %I', n.nspname, c.relname, :'migrator_role')
FROM pg_catalog.pg_class AS c
JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p')
\gexec
SELECT format('ALTER SEQUENCE %I.%I OWNER TO %I', n.nspname, c.relname, :'migrator_role')
FROM pg_catalog.pg_class AS c
JOIN pg_catalog.pg_namespace AS n ON n.oid = c.relnamespace
WHERE n.nspname = 'public' AND c.relkind = 'S'
\gexec

REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :"app_role", :"backup_role";
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :"app_role", :"backup_role";
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO :"app_role";
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO :"app_role";
GRANT SELECT ON ALL TABLES IN SCHEMA public TO :"backup_role";
GRANT SELECT ON ALL SEQUENCES IN SCHEMA public TO :"backup_role";

ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON TABLES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON SEQUENCES FROM PUBLIC;
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO :"app_role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO :"app_role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    GRANT SELECT ON TABLES TO :"backup_role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    GRANT SELECT ON SEQUENCES TO :"backup_role";

SELECT format('ALTER ROLE %I IN DATABASE %I SET statement_timeout = %L', :'app_role', :'database_name', '30s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET lock_timeout = %L', :'app_role', :'database_name', '3s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET idle_in_transaction_session_timeout = %L', :'app_role', :'database_name', '60s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET statement_timeout = %L', :'migrator_role', :'database_name', '300s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET lock_timeout = %L', :'migrator_role', :'database_name', '30s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET idle_in_transaction_session_timeout = %L', :'migrator_role', :'database_name', '300s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET statement_timeout = %L', :'backup_role', :'database_name', '120s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET lock_timeout = %L', :'backup_role', :'database_name', '5s')
UNION ALL SELECT format('ALTER ROLE %I IN DATABASE %I SET idle_in_transaction_session_timeout = %L', :'backup_role', :'database_name', '60s')
\gexec

COMMIT;
