\set ON_ERROR_STOP on

SELECT 1 / (current_database() = :'database_name')::integer;

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
        rolcanlogin AND NOT rolsuper AND NOT rolcreatedb AND NOT rolcreaterole
        AND NOT rolreplication AND NOT rolbypassrls
    ),
    'roles must exist and be least privilege'
)
FROM pg_catalog.pg_roles
WHERE rolname IN (:'app_role', :'migrator_role', :'backup_role');

SELECT pg_temp.assert_role_policy(
    has_database_privilege(:'migrator_role', :'database_name', 'CREATE,TEMPORARY')
    AND NOT has_database_privilege(:'app_role', :'database_name', 'CREATE')
    AND NOT has_database_privilege(:'backup_role', :'database_name', 'CREATE'),
    'database DDL authority must be restricted to migrator_role'
);

SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_tables
        WHERE schemaname = 'public'
          AND NOT has_table_privilege(:'app_role', format('%I.%I', schemaname, tablename), 'SELECT,INSERT,UPDATE,DELETE')
    ),
    'app_role lacks table DML'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_tables
        WHERE schemaname = 'public'
          AND (
              NOT has_table_privilege(:'backup_role', format('%I.%I', schemaname, tablename), 'SELECT')
              OR has_table_privilege(:'backup_role', format('%I.%I', schemaname, tablename), 'INSERT,UPDATE,DELETE')
          )
    ),
    'backup_role table privileges are not read only'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_sequences
        WHERE schemaname = 'public'
          AND NOT has_sequence_privilege(:'app_role', format('%I.%I', schemaname, sequencename), 'USAGE,SELECT,UPDATE')
    ),
    'app_role lacks sequence privileges'
);
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM pg_catalog.pg_sequences
        WHERE schemaname = 'public'
          AND (
              NOT has_sequence_privilege(:'backup_role', format('%I.%I', schemaname, sequencename), 'SELECT')
              OR has_sequence_privilege(:'backup_role', format('%I.%I', schemaname, sequencename), 'USAGE,UPDATE')
          )
    ),
    'backup_role sequence privileges are not read only'
);

WITH expected(role_name, object_type, privileges) AS (
    VALUES
        (:'app_role', 'r', ARRAY['SELECT', 'INSERT', 'UPDATE', 'DELETE']),
        (:'app_role', 'S', ARRAY['USAGE', 'SELECT', 'UPDATE']),
        (:'backup_role', 'r', ARRAY['SELECT']),
        (:'backup_role', 'S', ARRAY['SELECT'])
), actual AS (
    SELECT grantee.rolname AS role_name, defaults.defaclobjtype::text AS object_type,
           array_agg(DISTINCT acl.privilege_type) AS privileges
    FROM pg_catalog.pg_default_acl AS defaults
    JOIN pg_catalog.pg_roles AS owner ON owner.oid = defaults.defaclrole
    JOIN pg_catalog.pg_namespace AS namespace ON namespace.oid = defaults.defaclnamespace
    CROSS JOIN LATERAL aclexplode(
        COALESCE(defaults.defaclacl, acldefault(defaults.defaclobjtype, defaults.defaclrole))
    ) AS acl
    JOIN pg_catalog.pg_roles AS grantee ON grantee.oid = acl.grantee
    WHERE owner.rolname = :'migrator_role' AND namespace.nspname = 'public'
    GROUP BY grantee.rolname, defaults.defaclobjtype
)
SELECT pg_temp.assert_role_policy(
    NOT EXISTS (
        SELECT 1 FROM expected
        LEFT JOIN actual USING (role_name, object_type)
        WHERE NOT expected.privileges <@ actual.privileges
    ),
    'default table or sequence privileges are incomplete'
);

SELECT pg_temp.assert_role_policy(
    count(*) = 3 AND bool_and(setconfig @> required),
    'database role timeout policy is incomplete'
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
