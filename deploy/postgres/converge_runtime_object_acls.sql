\set ON_ERROR_STOP on

-- Required psql variables: database_name, app_role, migrator_role, backup_role.
-- This include is transaction-neutral: bootstrap owns its transaction and the
-- steady-state runner invokes psql with --single-transaction.
SELECT 1 / (current_database() = :'database_name')::integer;
SELECT 1 / (
    :'app_role' <> :'migrator_role'
    AND :'app_role' <> :'backup_role'
    AND :'migrator_role' <> :'backup_role'
)::integer;
SELECT 1 / (
    (SELECT count(*) = 3
     FROM pg_catalog.pg_roles
     WHERE rolname IN (:'app_role', :'migrator_role', :'backup_role'))
)::integer;
SELECT 1 / (
    COALESCE(
        (SELECT rolname = :'migrator_role' OR rolsuper
         FROM pg_catalog.pg_roles
         WHERE rolname = current_user),
        false
    )
)::integer;
SELECT 1 / (
    COALESCE(
        (SELECT owner.rolname = :'migrator_role'
         FROM pg_catalog.pg_namespace AS namespace
         JOIN pg_catalog.pg_roles AS owner ON owner.oid = namespace.nspowner
         WHERE namespace.nspname = 'public'),
        false
    )
)::integer;

\ir app_acl_policy.sql

-- One governed relation drives bootstrap and post-migration convergence.
-- Extension members and migration-control objects are deliberately excluded.
CREATE TEMP TABLE governed_objects ON COMMIT DROP AS
SELECT object.oid, object.relnamespace, object.relname, object.relkind,
       object.relowner, object.relacl
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE namespace.nspname = 'public'
  AND object.relkind IN ('r', 'p', 'S')
  AND object.relname NOT IN ('alembic_version', 'spatial_ref_sys')
  AND NOT EXISTS (
      SELECT 1 FROM pg_catalog.pg_depend AS dependency
      WHERE dependency.classid = 'pg_class'::regclass
        AND dependency.objid = object.oid AND dependency.deptype = 'e'
  );
CREATE UNIQUE INDEX governed_objects_oid_idx ON governed_objects (oid);

-- alembic_version is migration control rather than an application-governed
-- table, but a complete logical backup must still be able to lock and read it.
-- Rebuild its non-owner ACL separately so the app remains denied and pg_dump
-- can capture the database revision through the dedicated backup role.
SELECT format(
    'REVOKE ALL PRIVILEGES ON TABLE %I.%I FROM %s CASCADE',
    namespace.nspname,
    object.relname,
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM pg_catalog.pg_class AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
CROSS JOIN LATERAL aclexplode(
    COALESCE(object.relacl, acldefault('r'::"char", object.relowner))
) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE namespace.nspname = 'public'
  AND object.relname = 'alembic_version'
  AND object.relkind IN ('r', 'p')
  AND acl.grantee <> object.relowner
GROUP BY namespace.nspname, object.relname, acl.grantee, grantee.rolname
ORDER BY acl.grantee
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
  AND object.relname = 'alembic_version'
  AND object.relkind IN ('r', 'p')
\gexec

-- Migrations run as the named owner. A wrong-owner object is not silently
-- repaired with elevated credentials during deploy; convergence refuses.
SELECT 1 / (
    NOT EXISTS (
        SELECT 1
        FROM governed_objects AS object
        JOIN pg_catalog.pg_roles AS owner ON owner.oid = object.relowner
        WHERE owner.rolname <> :'migrator_role'
    )
)::integer;

-- A policy entry may name a table that has not reached this branch yet. Once
-- that table exists, however, every declared column must exist exactly; a
-- typo or an incomplete migration refuses the whole transactional rebuild.
SELECT 1 / (
    NOT EXISTS (
        SELECT 1
        FROM governed_objects AS object
        JOIN app_column_policy AS policy ON policy.table_name = object.relname
        LEFT JOIN pg_catalog.pg_attribute AS attribute
          ON attribute.attrelid = object.oid
         AND attribute.attname = policy.column_name
         AND attribute.attnum > 0
         AND NOT attribute.attisdropped
        WHERE object.relkind IN ('r', 'p')
          AND attribute.attrelid IS NULL
    )
)::integer;

-- Reconstruct every governed object's ACL from zero non-owner authority.
-- CASCADE removes grants delegated from stale grant/admin options.
SELECT format(
    'REVOKE ALL PRIVILEGES ON %s %I.%I FROM %s CASCADE',
    CASE WHEN object.relkind = 'S' THEN 'SEQUENCE' ELSE 'TABLE' END,
    namespace.nspname,
    object.relname,
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
CROSS JOIN LATERAL aclexplode(
    COALESCE(
        object.relacl,
        acldefault(
            CASE WHEN object.relkind = 'S' THEN 'S'::"char" ELSE 'r'::"char" END,
            object.relowner
        )
    )
) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE acl.grantee <> object.relowner
GROUP BY namespace.nspname, object.relname, object.relkind, acl.grantee, grantee.rolname
ORDER BY namespace.nspname, object.relname, object.relkind, acl.grantee
\gexec

-- Column ACLs are independent of table ACLs. Remove every explicit governed
-- column grant, including PUBLIC and grant-option dependents, before rebuild.
SELECT format(
    'REVOKE ALL PRIVILEGES (%I) ON TABLE %I.%I FROM %s CASCADE',
    attribute.attname,
    namespace.nspname,
    object.relname,
    CASE WHEN acl.grantee = 0 THEN 'PUBLIC' ELSE format('%I', grantee.rolname) END
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
JOIN pg_catalog.pg_attribute AS attribute ON attribute.attrelid = object.oid
CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl
LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
WHERE object.relkind IN ('r', 'p')
  AND attribute.attnum > 0
  AND NOT attribute.attisdropped
GROUP BY namespace.nspname, object.relname, attribute.attnum, attribute.attname,
         acl.grantee, grantee.rolname
ORDER BY namespace.nspname, object.relname, attribute.attnum, acl.grantee
\gexec

-- Unknown tables receive no app privilege. A declared future table becomes
-- active only after a migration creates it and this convergence runs.
SELECT format(
    'GRANT %s ON TABLE %I.%I TO %I',
    array_to_string(policy.privileges, ', '),
    namespace.nspname,
    object.relname,
    :'app_role'
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
JOIN app_table_policy AS policy ON policy.table_name = object.relname
WHERE object.relkind IN ('r', 'p')
\gexec

SELECT format(
    'GRANT %s (%s) ON TABLE %I.%I TO %I',
    policy.privilege_type,
    string_agg(format('%I', attribute.attname), ', ' ORDER BY attribute.attnum),
    namespace.nspname,
    object.relname,
    :'app_role'
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
JOIN app_column_policy AS policy ON policy.table_name = object.relname
JOIN pg_catalog.pg_attribute AS attribute
  ON attribute.attrelid = object.oid
 AND attribute.attname = policy.column_name
 AND attribute.attnum > 0
 AND NOT attribute.attisdropped
WHERE object.relkind IN ('r', 'p')
GROUP BY namespace.nspname, object.relname, policy.privilege_type
ORDER BY namespace.nspname, object.relname, policy.privilege_type
\gexec

SELECT format(
    'GRANT SELECT ON TABLE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'backup_role'
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE object.relkind IN ('r', 'p')
\gexec

SELECT format(
    'GRANT %s ON SEQUENCE %I.%I TO %I',
    array_to_string(policy.privileges, ', '),
    namespace.nspname,
    object.relname,
    :'app_role'
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
JOIN app_sequence_policy AS policy ON policy.sequence_name = object.relname
WHERE object.relkind = 'S'
\gexec

SELECT format(
    'GRANT SELECT ON SEQUENCE %I.%I TO %I',
    namespace.nspname,
    object.relname,
    :'backup_role'
)
FROM governed_objects AS object
JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
WHERE object.relkind = 'S'
\gexec
