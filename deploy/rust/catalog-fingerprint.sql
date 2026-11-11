-- Catalog fingerprint projection (Phase 10 plan section 8.3).
--
-- One read-only query that returns an ordered text projection of the
-- application schema. The fingerprint is the SHA-256 of these lines joined by
-- "\n" (no trailing newline). `migrate` and the baseline tests read this
-- file, so the projection has one definition.
--
-- Included: application schemas and their owner slot; tables, views,
-- materialized views and sequences with their persistence; columns (type,
-- nullability, default); constraints; indexes; functions and triggers; enum
-- labels in order; extension names and versions. Agent tables and their audit
-- sequence use this same projection; no agent object is excluded.
-- The schema owner slot is one normalized placeholder, not the live owner
-- name: one baseline manifest serves production and staging (plan section
-- 8.3).
-- Excluded: alembic_version, _sqlx_migrations, temporary objects, objects that
-- belong to an extension, ACLs, column owners, statistics and physical layout.
-- Run it inside one REPEATABLE READ transaction so every part sees one snapshot.
WITH extension_members AS (
    SELECT classid, objid FROM pg_depend WHERE deptype = 'e'
),
app_schemas AS (
    SELECT n.oid, n.nspname
    FROM pg_namespace n
    WHERE n.nspname NOT LIKE 'pg\_%'
      AND n.nspname <> 'information_schema'
      AND NOT EXISTS (
          SELECT 1 FROM extension_members e
          WHERE e.classid = 'pg_namespace'::regclass AND e.objid = n.oid
      )
),
app_relations AS (
    SELECT c.oid, s.nspname, c.relname, c.relkind, c.relpersistence
    FROM pg_class c
    JOIN app_schemas s ON s.oid = c.relnamespace
    WHERE c.relkind IN ('r', 'p', 'v', 'm', 'S', 'f')
      AND c.relpersistence <> 't'
      AND c.relname NOT IN ('alembic_version', '_sqlx_migrations')
      AND NOT EXISTS (
          SELECT 1 FROM extension_members e
          WHERE e.classid = 'pg_class'::regclass AND e.objid = c.oid
      )
),
app_functions AS (
    SELECT p.oid, s.nspname, p.proname
    FROM pg_proc p
    JOIN app_schemas s ON s.oid = p.pronamespace
    WHERE NOT EXISTS (
        SELECT 1 FROM extension_members e
        WHERE e.classid = 'pg_proc'::regclass AND e.objid = p.oid
    )
),
app_types AS (
    SELECT t.oid, s.nspname, t.typname
    FROM pg_type t
    JOIN app_schemas s ON s.oid = t.typnamespace
    WHERE t.typtype = 'e'
      AND NOT EXISTS (
          SELECT 1 FROM extension_members e
          WHERE e.classid = 'pg_type'::regclass AND e.objid = t.oid
      )
),
lines AS (
    SELECT format('schema|%s|owner=migrator', nspname) AS line
    FROM app_schemas
    UNION ALL
    SELECT format('relation|%s.%s|kind=%s|persistence=%s', nspname, relname, relkind, relpersistence)
    FROM app_relations
    UNION ALL
    SELECT format(
        'column|%s.%s|%s|type=%s|not_null=%s|default=%s',
        r.nspname, r.relname, a.attname, format_type(a.atttypid, a.atttypmod),
        a.attnotnull, COALESCE(pg_get_expr(d.adbin, d.adrelid), '')
    )
    FROM app_relations r
    JOIN pg_attribute a ON a.attrelid = r.oid AND a.attnum > 0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
    UNION ALL
    SELECT format(
        'constraint|%s.%s|%s|type=%s|def=%s|deferrable=%s|deferred=%s',
        r.nspname, r.relname, con.conname, con.contype, pg_get_constraintdef(con.oid, true),
        con.condeferrable, con.condeferred
    )
    FROM app_relations r
    JOIN pg_constraint con ON con.conrelid = r.oid
    UNION ALL
    SELECT format(
        'index|%s.%s|%s|def=%s|unique=%s|valid=%s',
        r.nspname, r.relname, i.relname, pg_get_indexdef(x.indexrelid), x.indisunique, x.indisvalid
    )
    FROM app_relations r
    JOIN pg_index x ON x.indrelid = r.oid
    JOIN pg_class i ON i.oid = x.indexrelid
    UNION ALL
    SELECT format(
        'function|%s.%s(%s)|returns=%s|language=%s|body_md5=%s',
        f.nspname, f.proname, pg_get_function_identity_arguments(p.oid),
        pg_get_function_result(p.oid), l.lanname, md5(COALESCE(p.prosrc, ''))
    )
    FROM app_functions f
    JOIN pg_proc p ON p.oid = f.oid
    JOIN pg_language l ON l.oid = p.prolang
    UNION ALL
    SELECT format('trigger|%s.%s|%s|def=%s', r.nspname, r.relname, t.tgname, pg_get_triggerdef(t.oid, true))
    FROM app_relations r
    JOIN pg_trigger t ON t.tgrelid = r.oid AND NOT t.tgisinternal
    UNION ALL
    SELECT format(
        'enum|%s.%s|labels=%s',
        ty.nspname, ty.typname,
        (SELECT string_agg(e.enumlabel, ',' ORDER BY e.enumsortorder) FROM pg_enum e WHERE e.enumtypid = ty.oid)
    )
    FROM app_types ty
    UNION ALL
    SELECT format('extension|%s|version=%s', extname, extversion)
    FROM pg_extension
)
SELECT line FROM lines ORDER BY line COLLATE "C";
