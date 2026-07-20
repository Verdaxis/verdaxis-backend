\set ON_ERROR_STOP on

SELECT 1 / (current_database() = :'database_name')::integer;
SELECT 1 / (
    :'app_role' <> :'migrator_role'
    AND :'app_role' <> :'backup_role'
    AND :'migrator_role' <> :'backup_role'
)::integer;

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

CREATE TEMP VIEW app_policy_objects AS
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
        SELECT 1 FROM app_policy_objects AS object
        JOIN pg_catalog.pg_roles AS owner ON owner.oid = object.relowner
        WHERE owner.rolname <> :'migrator_role'
    ),
    'every app-owned object must be owned by migrator_role'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM app_policy_objects AS object
        WHERE object.relkind IN ('r', 'p') AND (
            NOT has_table_privilege(:'app_role', object.oid, 'SELECT,INSERT,UPDATE,DELETE')
            OR has_table_privilege(:'app_role', object.oid, 'TRUNCATE,REFERENCES,TRIGGER')
            OR NOT has_table_privilege(:'backup_role', object.oid, 'SELECT')
            OR has_table_privilege(:'backup_role', object.oid, 'INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER')
        )
    ),
    'app-object table privileges are not exact'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM app_policy_objects AS object
        WHERE object.relkind = 'S' AND (
            NOT has_sequence_privilege(:'app_role', object.oid, 'USAGE,SELECT,UPDATE')
            OR NOT has_sequence_privilege(:'backup_role', object.oid, 'SELECT')
            OR has_sequence_privilege(:'backup_role', object.oid, 'USAGE,UPDATE')
        )
    ),
    'app-object sequence privileges are not exact'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM app_policy_objects AS object
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
        WHERE acl.grantee = 0
           OR grantee.rolname NOT IN (:'app_role', :'migrator_role', :'backup_role')
    ),
    'app-owned objects contain grants to an unexpected role or PUBLIC'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1
        FROM pg_catalog.pg_class AS object
        JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = object.relnamespace
        WHERE namespace.nspname = 'public'
          AND object.relkind IN ('r', 'p')
          AND (
              object.relname IN ('alembic_version', 'spatial_ref_sys')
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
    'control or extension tables are mutable by app or backup roles'
);

WITH expected(role_name, object_type, privilege_type, is_grantable, grantor_name) AS (
    VALUES
        (:'app_role', 'r', 'SELECT', false, :'migrator_role'),
        (:'app_role', 'r', 'INSERT', false, :'migrator_role'),
        (:'app_role', 'r', 'UPDATE', false, :'migrator_role'),
        (:'app_role', 'r', 'DELETE', false, :'migrator_role'),
        (:'app_role', 'S', 'USAGE', false, :'migrator_role'),
        (:'app_role', 'S', 'SELECT', false, :'migrator_role'),
        (:'app_role', 'S', 'UPDATE', false, :'migrator_role'),
        (:'backup_role', 'r', 'SELECT', false, :'migrator_role'),
        (:'backup_role', 'S', 'SELECT', false, :'migrator_role')
), actual AS (
    SELECT COALESCE(grantee.rolname, 'PUBLIC') AS role_name,
           defaults.defaclobjtype::text AS object_type,
           acl.privilege_type,
           acl.is_grantable,
           grantor.rolname AS grantor_name
    FROM pg_catalog.pg_default_acl AS defaults
    JOIN pg_catalog.pg_roles AS owner ON owner.oid = defaults.defaclrole
    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = defaults.defaclnamespace
    CROSS JOIN LATERAL aclexplode(defaults.defaclacl) AS acl
    LEFT JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
    JOIN pg_catalog.pg_roles AS grantor ON grantor.oid = acl.grantor
    WHERE owner.rolname = :'migrator_role' AND namespace.nspname = 'public'
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
