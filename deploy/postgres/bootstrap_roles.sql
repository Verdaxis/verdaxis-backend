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
SELECT format('REVOKE %I FROM %I CASCADE', granted.rolname, member.rolname)
FROM pg_catalog.pg_auth_members AS membership
JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
WHERE granted.rolname IN (:'app_role', :'migrator_role', :'backup_role')
   OR member.rolname IN (:'app_role', :'migrator_role', :'backup_role')
ORDER BY granted.rolname, member.rolname
\gexec

ALTER DATABASE :"database_name" OWNER TO :"migrator_role";
-- Revoke every explicit non-owner database ACL entry, including unrelated
-- roles. Ownership is the only unavoidable authority and is the migrator.
SELECT DISTINCT format(
    'REVOKE ALL PRIVILEGES ON DATABASE %I FROM %s CASCADE',
    :'database_name',
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM pg_catalog.pg_database AS database
CROSS JOIN LATERAL aclexplode(
    COALESCE(database.datacl, acldefault('d', database.datdba))
) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE database.datname = :'database_name'
  AND acl.grantee <> database.datdba
ORDER BY 1
\gexec
REVOKE ALL ON DATABASE :"database_name" FROM PUBLIC CASCADE;
GRANT CONNECT ON DATABASE :"database_name" TO :"app_role", :"backup_role";

ALTER SCHEMA public OWNER TO :"migrator_role";
-- Apply the same complete-ACL reset to public. pg_database_owner is not an
-- extra allowlisted grantee after ownership transfer: it resolves to the same
-- migrator authority and any explicit ACL entry is removed.
SELECT DISTINCT format(
    'REVOKE ALL PRIVILEGES ON SCHEMA public FROM %s CASCADE',
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM pg_catalog.pg_namespace AS namespace
CROSS JOIN LATERAL aclexplode(
    COALESCE(namespace.nspacl, acldefault('n', namespace.nspowner))
) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE namespace.nspname = 'public'
  AND acl.grantee <> namespace.nspowner
ORDER BY 1
\gexec
REVOKE ALL ON SCHEMA public FROM PUBLIC CASCADE;
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

-- Remove app/backup authority from control and extension objects as well as
-- governed objects. Exact app/backup grants are rebuilt only for governed
-- objects by the shared owner-executable convergence include.
REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :"app_role", :"backup_role" CASCADE;
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :"app_role", :"backup_role" CASCADE;

\ir converge_runtime_object_acls.sql

-- Default privileges compose global rows with per-schema rows. Reset every
-- PostgreSQL 17 default-ACL object class for every protected owner. Every ACL
-- entry is removed (including stale owner grant options), then global
-- hard-wired defaults and the intended additive public policy are rebuilt.
SELECT DISTINCT format(
    'ALTER DEFAULT PRIVILEGES FOR ROLE %I%s REVOKE ALL PRIVILEGES ON %s FROM %s CASCADE',
    owner.rolname,
    CASE
        WHEN defaults.defaclnamespace = 0 THEN ''
        ELSE ' IN SCHEMA public'
    END,
    CASE defaults.defaclobjtype
        WHEN 'r' THEN 'TABLES'
        WHEN 'S' THEN 'SEQUENCES'
        WHEN 'f' THEN 'FUNCTIONS'
        WHEN 'T' THEN 'TYPES'
        WHEN 'n' THEN 'SCHEMAS'
    END,
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM pg_catalog.pg_default_acl AS defaults
JOIN pg_catalog.pg_roles AS owner ON owner.oid = defaults.defaclrole
LEFT JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = defaults.defaclnamespace
CROSS JOIN LATERAL aclexplode(defaults.defaclacl) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE owner.rolname IN (:'app_role', :'migrator_role', :'backup_role')
  AND (defaults.defaclnamespace = 0 OR namespace.nspname = 'public')
  AND defaults.defaclobjtype IN ('r', 'S', 'f', 'T', 'n')
  AND (defaults.defaclnamespace = 0 OR defaults.defaclobjtype <> 'n')
ORDER BY 1
\gexec

-- Restore each global ACL to PostgreSQL's hard-wired owner/PUBLIC defaults.
-- PostgreSQL removes pg_default_acl rows once they equal those defaults.
SELECT format(
    'ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT ALL PRIVILEGES ON %s TO %I',
    owner_name,
    object_type,
    owner_name
)
FROM (VALUES (:'app_role'), (:'migrator_role'), (:'backup_role')) AS owners(owner_name)
CROSS JOIN (VALUES ('TABLES'), ('SEQUENCES'), ('FUNCTIONS'), ('TYPES'), ('SCHEMAS'))
    AS object_types(object_type)
\gexec
SELECT format(
    'ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT EXECUTE ON FUNCTIONS TO PUBLIC',
    owner_name
)
FROM (VALUES (:'app_role'), (:'migrator_role'), (:'backup_role')) AS owners(owner_name)
\gexec
SELECT format(
    'ALTER DEFAULT PRIVILEGES FOR ROLE %I GRANT USAGE ON TYPES TO PUBLIC',
    owner_name
)
FROM (VALUES (:'app_role'), (:'migrator_role'), (:'backup_role')) AS owners(owner_name)
\gexec

ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON TABLES FROM PUBLIC, :"app_role", :"backup_role" CASCADE;
ALTER DEFAULT PRIVILEGES FOR ROLE :"migrator_role" IN SCHEMA public
    REVOKE ALL ON SEQUENCES FROM PUBLIC, :"app_role", :"backup_role" CASCADE;
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
