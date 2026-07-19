\set ON_ERROR_STOP on

-- Required psql variables: database_name, app_role, migrator_role, backup_role.
-- Passwords are deliberately absent; provision them through the secret manager.
SELECT 1 / (current_database() = :'database_name')::integer;
SELECT 1 / (
    :'app_role' <> :'migrator_role'
    AND :'app_role' <> :'backup_role'
    AND :'migrator_role' <> :'backup_role'
)::integer;

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

-- NOINHERIT does not block SET ROLE. Remove every membership edge involving
-- a protected role so none can escalate or be assumed through membership.
SELECT format('REVOKE %I FROM %I', granted.rolname, member.rolname)
FROM pg_catalog.pg_auth_members AS membership
JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
WHERE granted.rolname IN (:'app_role', :'migrator_role', :'backup_role')
   OR member.rolname IN (:'app_role', :'migrator_role', :'backup_role')
\gexec

ALTER DATABASE :"database_name" OWNER TO :"migrator_role";
REVOKE ALL ON DATABASE :"database_name" FROM PUBLIC, :"app_role", :"backup_role";
GRANT CONNECT ON DATABASE :"database_name" TO :"app_role", :"backup_role";

ALTER SCHEMA public OWNER TO :"migrator_role";
REVOKE ALL ON SCHEMA public FROM PUBLIC, :"app_role", :"backup_role";
GRANT USAGE ON SCHEMA public TO :"app_role", :"backup_role";

-- App-owned objects are public ordinary/partitioned tables and sequences that
-- are not extension members or migration-control objects.
SELECT format(
    'ALTER %s %I.%I OWNER TO %I',
    CASE WHEN object.relkind = 'S' THEN 'SEQUENCE' ELSE 'TABLE' END,
    namespace.nspname,
    object.relname,
    :'migrator_role'
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind IN ('r', 'p', 'S')
  AND object.relname NOT IN ('alembic_version', 'spatial_ref_sys')
  AND NOT EXISTS (
      SELECT 1
      FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid
        AND dependency.deptype = 'e'
  )
\gexec

-- Remove stale direct privileges everywhere, then grant only on app objects.
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :"app_role", :"backup_role";
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :"app_role", :"backup_role";

SELECT format(
    'GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'app_role'
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind IN ('r', 'p')
  AND object.relname NOT IN ('alembic_version', 'spatial_ref_sys')
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid AND dependency.deptype = 'e'
  )
\gexec
SELECT format(
    'GRANT SELECT ON TABLE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'backup_role'
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind IN ('r', 'p')
  AND object.relname NOT IN ('alembic_version', 'spatial_ref_sys')
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid AND dependency.deptype = 'e'
  )
\gexec
SELECT format(
    'GRANT USAGE, SELECT, UPDATE ON SEQUENCE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'app_role'
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind = 'S'
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid AND dependency.deptype = 'e'
  )
\gexec
SELECT format(
    'GRANT SELECT ON SEQUENCE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'backup_role'
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind = 'S'
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid AND dependency.deptype = 'e'
  )
\gexec

-- Remove every stale explicit default ACL owned by the migrator, including
-- grants to roles other than the three policy roles, before recreating the
-- exact table/sequence defaults.
SELECT DISTINCT format(
    'ALTER DEFAULT PRIVILEGES FOR ROLE %I IN SCHEMA public REVOKE ALL ON %s FROM %s',
    :'migrator_role',
    CASE defaults.defaclobjtype
        WHEN 'r' THEN 'TABLES'
        WHEN 'S' THEN 'SEQUENCES'
        WHEN 'f' THEN 'FUNCTIONS'
        WHEN 'T' THEN 'TYPES'
    END,
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM pg_catalog.pg_default_acl AS defaults
JOIN pg_catalog.pg_roles AS owner ON owner.oid = defaults.defaclrole
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = defaults.defaclnamespace
CROSS JOIN LATERAL aclexplode(defaults.defaclacl) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE owner.rolname = :'migrator_role'
  AND namespace.nspname = 'public'
  AND defaults.defaclobjtype IN ('r', 'S', 'f', 'T')
\gexec

ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON TABLES FROM PUBLIC, :"app_role", :"backup_role";
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON SEQUENCES FROM PUBLIC, :"app_role", :"backup_role";
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
