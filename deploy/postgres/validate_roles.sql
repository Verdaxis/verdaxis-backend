\set ON_ERROR_STOP on

SELECT 1 / (current_database() = :'database_name')::integer;
SELECT 1 / (
    :'app_role' <> :'migrator_role'
    AND :'app_role' <> :'backup_role'
    AND :'migrator_role' <> :'backup_role'
)::integer;

\ir app_acl_policy.sql

CREATE OR REPLACE FUNCTION pg_temp.assert_role_policy(ok boolean, message text)
RETURNS void LANGUAGE plpgsql AS $$
BEGIN
    IF NOT COALESCE(ok, false) THEN
        RAISE EXCEPTION 'role policy validation failed: %', message;
    END IF;
END;
$$;

SELECT pg_temp.assert_role_policy(
    count(*) = 3 AND bool_and(
        rolcanlogin AND NOT rolinherit AND NOT rolsuper AND NOT rolcreatedb
        AND NOT rolcreaterole AND NOT rolreplication AND NOT rolbypassrls
    ),
    'roles must exist with exact least-privilege properties'
)
FROM pg_catalog.pg_roles
WHERE rolname IN (:'app_role', :'migrator_role', :'backup_role');

SELECT pg_temp.assert_role_policy(
    (
        SELECT owner.rolname = :'migrator_role'
        FROM pg_catalog.pg_class AS object
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
        JOIN pg_catalog.pg_roles AS owner ON owner.oid = object.relowner
        WHERE namespace.nspname = 'public'
          AND object.relname = 'alembic_version'
          AND object.relkind IN ('r', 'p')
    ),
    'alembic_version migration-control table has wrong owner'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        WITH actual AS (
            SELECT COALESCE(grantee.rolname, 'PUBLIC') AS grantee_name,
                   acl.privilege_type,
                   acl.is_grantable,
                   grantor.rolname AS grantor_name
            FROM pg_catalog.pg_class AS object
            CROSS JOIN LATERAL aclexplode(
                COALESCE(object.relacl, acldefault('r'::"char", object.relowner))
            ) AS acl
            LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
            JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
            WHERE object.oid = 'public.alembic_version'::regclass
              AND acl.grantee <> object.relowner
        ), expected(grantee_name, privilege_type, is_grantable, grantor_name) AS (
            VALUES (:'backup_role', 'SELECT', false, :'migrator_role')
        ), differences AS (
            (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
            UNION ALL
            (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
        )
        SELECT 1 FROM differences
    ),
    'alembic_version must grant exact read-only backup authority'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_auth_members AS membership
        JOIN pg_catalog.pg_roles AS granted ON granted.oid = membership.roleid
        JOIN pg_catalog.pg_roles AS member ON member.oid = membership.member
        WHERE granted.rolname IN (:'app_role', :'migrator_role', :'backup_role')
           OR member.rolname IN (:'app_role', :'migrator_role', :'backup_role')
    ),
    'protected roles must have no memberships or SET ROLE path'
);

SELECT pg_temp.assert_role_policy(
    (SELECT owner.rolname
     FROM pg_catalog.pg_database AS database
     JOIN pg_catalog.pg_roles AS owner ON owner.oid = database.datdba
     WHERE database.datname = :'database_name') = :'migrator_role',
    'database owner must be migrator_role'
);
SELECT pg_temp.assert_role_policy(
    (SELECT owner.rolname
     FROM pg_catalog.pg_namespace AS namespace
     JOIN pg_catalog.pg_roles AS owner ON owner.oid = namespace.nspowner
     WHERE namespace.nspname = 'public') = :'migrator_role',
    'public schema owner must be migrator_role'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        WITH expected(grantee_name, privilege_type, is_grantable, grantor_name) AS (
            VALUES
                (:'migrator_role', 'CREATE', false, :'migrator_role'),
                (:'migrator_role', 'CONNECT', false, :'migrator_role'),
                (:'migrator_role', 'TEMPORARY', false, :'migrator_role'),
                (:'app_role', 'CONNECT', false, :'migrator_role'),
                (:'backup_role', 'CONNECT', false, :'migrator_role')
        ), actual AS (
            SELECT COALESCE(grantee.rolname, 'PUBLIC') AS grantee_name,
                   acl.privilege_type,
                   acl.is_grantable,
                   grantor.rolname AS grantor_name
            FROM pg_catalog.pg_database AS database
            CROSS JOIN LATERAL aclexplode(
                COALESCE(database.datacl, acldefault('d', database.datdba))
            ) AS acl
            LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
            JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
            WHERE database.datname = :'database_name'
        ), differences AS (
            (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
            UNION ALL
            (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
        )
        SELECT 1 FROM differences
    ),
    'database ACL must exactly match owner, app, and backup policy'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        WITH expected(grantee_name, privilege_type, is_grantable, grantor_name) AS (
            VALUES
                (:'migrator_role', 'CREATE', false, :'migrator_role'),
                (:'migrator_role', 'USAGE', false, :'migrator_role'),
                (:'app_role', 'USAGE', false, :'migrator_role'),
                (:'backup_role', 'USAGE', false, :'migrator_role')
        ), actual AS (
            SELECT COALESCE(grantee.rolname, 'PUBLIC') AS grantee_name,
                   acl.privilege_type,
                   acl.is_grantable,
                   grantor.rolname AS grantor_name
            FROM pg_catalog.pg_namespace AS namespace
            CROSS JOIN LATERAL aclexplode(
                COALESCE(namespace.nspacl, acldefault('n', namespace.nspowner))
            ) AS acl
            LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
            JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
            WHERE namespace.nspname = 'public'
        ), differences AS (
            (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
            UNION ALL
            (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
        )
        SELECT 1 FROM differences
    ),
    'public schema ACL must exactly match owner, app, and backup policy'
);

CREATE TEMP VIEW governed_objects AS
SELECT object.oid, object.relname, object.relkind, object.relowner, object.relacl
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

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM governed_objects AS object
        JOIN pg_catalog.pg_roles AS owner ON owner.oid = object.relowner
        WHERE owner.rolname <> :'migrator_role'
    ),
    'every app-owned object must be owned by migrator_role'
);

SELECT pg_temp.assert_role_policy(
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
    ),
    'every declared column for an existing table must exist exactly'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM governed_objects AS object
        CROSS JOIN (VALUES ('SELECT'), ('INSERT'), ('UPDATE'), ('DELETE'))
            AS privilege(privilege_type)
        LEFT JOIN app_table_policy AS policy ON policy.table_name = object.relname
        WHERE object.relkind IN ('r', 'p')
          AND has_table_privilege(
              :'app_role', object.oid, privilege.privilege_type
          ) <> (
              privilege.privilege_type = ANY(
                  COALESCE(policy.privileges, ARRAY[]::text[])
              )
          )
    ),
    'app table privileges do not exactly match the declarative policy'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM governed_objects AS object
        CROSS JOIN (VALUES ('USAGE'), ('SELECT'), ('UPDATE'))
            AS privilege(privilege_type)
        LEFT JOIN app_sequence_policy AS policy
          ON policy.sequence_name = object.relname
        WHERE object.relkind = 'S'
          AND has_sequence_privilege(
              :'app_role', object.oid, privilege.privilege_type
          ) <> (
              privilege.privilege_type = ANY(
                  COALESCE(policy.privileges, ARRAY[]::text[])
              )
          )
    ),
    'app sequence privileges do not exactly match the declarative policy'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        WITH actual AS (
            SELECT object.relname, object.relkind::text,
                   COALESCE(grantee.rolname, 'PUBLIC') AS grantee_name,
                   acl.privilege_type, acl.is_grantable,
                   grantor.rolname AS grantor_name
            FROM governed_objects AS object
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
            JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
            WHERE acl.grantee <> object.relowner
        ), expected AS (
            SELECT object.relname, object.relkind::text, :'app_role',
                   privilege.privilege_type, false, :'migrator_role'
            FROM governed_objects AS object
            JOIN app_table_policy AS policy ON policy.table_name = object.relname
            CROSS JOIN LATERAL unnest(policy.privileges) AS privilege(privilege_type)
            WHERE object.relkind IN ('r', 'p')
            UNION ALL
            SELECT object.relname, object.relkind::text, :'app_role',
                   privilege.privilege_type, false, :'migrator_role'
            FROM governed_objects AS object
            JOIN app_sequence_policy AS policy ON policy.sequence_name = object.relname
            CROSS JOIN LATERAL unnest(policy.privileges) AS privilege(privilege_type)
            WHERE object.relkind = 'S'
            UNION ALL
            SELECT object.relname, object.relkind::text, :'backup_role',
                   'SELECT', false, :'migrator_role'
            FROM governed_objects AS object
        ), differences AS (
            (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
            UNION ALL
            (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
        )
        SELECT 1 FROM differences
    ),
    'governed non-owner object ACLs do not exactly match policy'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        WITH actual AS (
            SELECT object.relname, attribute.attname,
                   COALESCE(grantee.rolname, 'PUBLIC') AS grantee_name,
                   acl.privilege_type, acl.is_grantable,
                   grantor.rolname AS grantor_name
            FROM governed_objects AS object
            JOIN pg_catalog.pg_attribute AS attribute ON attribute.attrelid = object.oid
            CROSS JOIN LATERAL aclexplode(attribute.attacl) AS acl
            LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
            JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
            WHERE object.relkind IN ('r', 'p')
              AND attribute.attnum > 0
              AND NOT attribute.attisdropped
        ), expected AS (
            SELECT object.relname, attribute.attname, :'app_role',
                   policy.privilege_type, false, :'migrator_role'
            FROM governed_objects AS object
            JOIN app_column_policy AS policy ON policy.table_name = object.relname
            JOIN pg_catalog.pg_attribute AS attribute
              ON attribute.attrelid = object.oid
             AND attribute.attname = policy.column_name
             AND attribute.attnum > 0
             AND NOT attribute.attisdropped
            WHERE object.relkind IN ('r', 'p')
        ), differences AS (
            (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
            UNION ALL
            (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
        )
        SELECT 1 FROM differences
    ),
    'governed column ACLs do not exactly match policy, including PUBLIC'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM governed_objects AS object
        JOIN pg_catalog.pg_attribute AS attribute ON attribute.attrelid = object.oid
        WHERE object.relkind IN ('r', 'p')
          AND attribute.attnum > 0
          AND NOT attribute.attisdropped
          AND has_column_privilege(
              :'backup_role', object.oid, attribute.attnum,
              'INSERT,UPDATE,REFERENCES'
          )
    ),
    'backup role must not inherit column write privileges'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS object
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
        WHERE namespace.nspname = 'public'
          AND object.relkind IN ('r', 'p')
          AND (
              object.relname IN (
                  'alembic_version', 'spatial_ref_sys', 'seed_runs',
                  'market_row_quarantines', 'organization_market_approvals',
                  'market_signal_ingestion_runs', 'market_indications',
                  'fair_price_bands', 'physical_stems'
              )
              OR EXISTS (
                  SELECT 1 FROM pg_catalog.pg_depend AS dependency
                  WHERE dependency.classid = 'pg_class'::regclass
                    AND dependency.objid = object.oid AND dependency.deptype = 'e'
              )
          )
          AND (
              has_table_privilege(:'app_role', object.oid, 'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
              OR has_table_privilege(:'backup_role', object.oid, 'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
          )
    ),
    'provenance, operator, control, or extension tables are mutable by app or backup roles'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS object
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
        WHERE namespace.nspname = 'public'
          AND object.relkind IN ('r', 'p')
          AND object.relname IN ('audit_logs', 'user_status_transitions')
          AND (
              has_table_privilege(
                  :'app_role', object.oid,
                  'UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER'
              )
              OR has_table_privilege(
                  :'backup_role', object.oid,
                  'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER'
              )
          )
    ),
    'audit and status history must remain append-only for app and read-only for backup'
);

WITH expected(
    owner_name,
    namespace_name,
    role_name,
    object_type,
    privilege_type,
    is_grantable,
    grantor_name
) AS (
    VALUES
        (:'migrator_role', 'public', :'backup_role', 'r', 'SELECT', false, :'migrator_role'),
        (:'migrator_role', 'public', :'backup_role', 'S', 'SELECT', false, :'migrator_role')
), actual AS (
    SELECT owner.rolname AS owner_name,
           CASE
               WHEN defaults.defaclnamespace = 0 THEN 'GLOBAL'
               ELSE namespace.nspname
           END AS namespace_name,
           COALESCE(grantee.rolname, 'PUBLIC') AS role_name,
           defaults.defaclobjtype::text AS object_type,
           acl.privilege_type,
           acl.is_grantable,
           grantor.rolname AS grantor_name
    FROM pg_catalog.pg_default_acl AS defaults
    JOIN pg_catalog.pg_roles AS owner ON owner.oid = defaults.defaclrole
    LEFT JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = defaults.defaclnamespace
    CROSS JOIN LATERAL aclexplode(defaults.defaclacl) AS acl
    LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
    JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
    WHERE owner.rolname IN (:'app_role', :'migrator_role', :'backup_role')
      AND (defaults.defaclnamespace = 0 OR namespace.nspname = 'public')
      AND defaults.defaclobjtype IN ('r', 'S', 'f', 'T', 'n')
), differences AS (
    (SELECT * FROM actual EXCEPT ALL SELECT * FROM expected)
    UNION ALL
    (SELECT * FROM expected EXCEPT ALL SELECT * FROM actual)
)
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (SELECT 1 FROM differences),
    'default privileges must exactly match app and backup policy'
);

SELECT pg_temp.assert_role_policy(
    count(*) = 3 AND bool_and(setconfig @> required AND setconfig <@ required),
    'database role timeout policy must be exact'
)
FROM (
    SELECT role_name, setconfig,
           CASE role_name
               WHEN :'app_role' THEN ARRAY['statement_timeout=30s', 'lock_timeout=3s', 'idle_in_transaction_session_timeout=60s']
               WHEN :'migrator_role' THEN ARRAY['statement_timeout=300s', 'lock_timeout=30s', 'idle_in_transaction_session_timeout=300s']
               ELSE ARRAY['statement_timeout=120s', 'lock_timeout=5s', 'idle_in_transaction_session_timeout=60s']
           END AS required
    FROM (
        SELECT roles.rolname AS role_name, settings.setconfig
        FROM pg_catalog.pg_db_role_setting AS settings
        JOIN pg_catalog.pg_roles AS roles ON roles.oid = settings.setrole
        JOIN pg_catalog.pg_database AS database ON database.oid = settings.setdatabase
        WHERE database.datname = :'database_name'
          AND roles.rolname IN (:'app_role', :'migrator_role', :'backup_role')
    ) AS configured
) AS policies;
