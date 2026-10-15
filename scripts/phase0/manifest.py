"""Read PostgreSQL catalogs for the frozen Phase 0 copy contract.

This module opens no connections and grants no import authority. Callers own
connection attestation and transactions. Catalog inspection is read-only.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any, TypedDict

from sqlalchemy import text

PROD_SHA = "c08266bd6969143812dec7c5033338428cd5f159"
PROD_REVISION = "kycg_20261014_prod"
STAGING_SHA = "b218ab264b92f99007bacc05d4871413e024b72a"
STAGING_REVISION = "kycd_20261015_staging"
MANIFEST_PATH = Path(__file__).resolve().parents[2] / "deploy/phase0/copy-manifest.json"


class CatalogManifest(TypedDict):
    revision: str
    tables: dict[str, Any]
    sequences: list[dict[str, Any]]
    extensions: list[dict[str, Any]]
    extension_members: list[dict[str, Any]]
    reference_rows: dict[str, list[dict[str, Any]]]


def load_manifest() -> dict:
    return json.loads(MANIFEST_PATH.read_text())


def digest(value: Any) -> str:
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def require_complete_inventory(actual, declared) -> None:
    """Refuse extra/missing tables, columns, or changed structural definitions."""
    if isinstance(actual, dict) and isinstance(declared, dict):
        # Reference values belong to the per-run data receipt, not the schema
        # fingerprint: migration-generated created_at values vary by run.
        actual = {key: value for key, value in actual.items() if key != "reference_rows"}
        declared = {key: value for key, value in declared.items() if key != "reference_rows"}
    if actual != declared:
        raise ValueError("Unclassified source schema; import refused")


def inspect_catalog(connection) -> CatalogManifest:
    """Inspect ordinary tables and partitions outside extension membership.

    Values in reference rows use PostgreSQL text, retaining decimal precision
    and SQL NULL. Timestamps use UTC. No application model is imported.
    """
    def rows(sql, **parameters):
        return [
            {key: value.decode() if isinstance(value, bytes) else value for key, value in row.items()}
            for row in connection.execute(text(sql), parameters).mappings()
        ]

    tables = {}
    relations = rows("""
        SELECT n.nspname AS schema, c.relname AS name, c.relkind AS kind,
               c.relispartition AS is_partition,
               pg_get_partkeydef(c.oid) AS partition_key,
               pg_get_expr(c.relpartbound, c.oid) AS partition_bound
        FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
        WHERE c.relkind IN ('r', 'p') AND n.nspname NOT IN ('pg_catalog', 'information_schema')
          AND n.nspname NOT LIKE 'pg_toast%'
          AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid = 'pg_class'::regclass
                          AND d.objid = c.oid AND d.deptype = 'e')
        ORDER BY n.nspname, c.relname
    """)
    for relation in relations:
        schema, name = relation["schema"], relation["name"]
        qualified = f'"{schema.replace(chr(34), chr(34)*2)}"."{name.replace(chr(34), chr(34)*2)}"'
        key = name if schema == "public" else f"{schema}.{name}"
        columns = rows("""
            SELECT a.attname AS name, format_type(a.atttypid, a.atttypmod) AS type,
                   a.attnotnull AS not_null, a.attidentity AS identity,
                   a.attgenerated AS generated, pg_get_expr(d.adbin, d.adrelid) AS default,
                   col_description(a.attrelid, a.attnum) AS comment
            FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
            WHERE a.attrelid=to_regclass(:table) AND a.attnum > 0 AND NOT a.attisdropped
            ORDER BY a.attnum
        """, table=qualified)
        constraints = rows("""
            SELECT conname AS name, contype AS kind, convalidated AS validated,
                   pg_get_constraintdef(oid, true) AS definition
            FROM pg_constraint WHERE conrelid=to_regclass(:table) ORDER BY conname
        """, table=qualified)
        indexes = rows("""
            SELECT c.relname AS name, pg_get_indexdef(i.indexrelid) AS definition,
                   i.indisvalid AS valid, i.indisready AS ready
            FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid
            WHERE i.indrelid=to_regclass(:table) ORDER BY c.relname
        """, table=qualified)
        triggers = rows("""
            SELECT t.tgname AS name, t.tgenabled AS enabled,
                   pg_get_triggerdef(t.oid, true) AS definition,
                   pg_get_functiondef(t.tgfoid) AS function
            FROM pg_trigger t WHERE t.tgrelid=to_regclass(:table) AND NOT t.tgisinternal
            ORDER BY t.tgname
        """, table=qualified)
        tables[key] = dict(relation, columns=columns, constraints=constraints, indexes=indexes, triggers=triggers)
    sequences = rows("""
        SELECT n.nspname AS schema, c.relname AS name, format_type(s.seqtypid, NULL) AS type,
               s.seqstart::text AS start, s.seqincrement::text AS increment,
               s.seqmax::text AS maximum, s.seqmin::text AS minimum,
               s.seqcache::text AS cache, s.seqcycle AS cycle,
               rn.nspname AS owner_schema, r.relname AS owner_table, a.attname AS owner_column
        FROM pg_sequence s JOIN pg_class c ON c.oid=s.seqrelid
        JOIN pg_namespace n ON n.oid=c.relnamespace
        LEFT JOIN pg_depend d ON d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype IN ('a','i')
        LEFT JOIN pg_class r ON r.oid=d.refobjid
        LEFT JOIN pg_namespace rn ON rn.oid=r.relnamespace
        LEFT JOIN pg_attribute a ON a.attrelid=r.oid AND a.attnum=d.refobjsubid
        WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
        ORDER BY n.nspname, c.relname
    """)
    extensions = rows("SELECT extname AS name, extversion AS version FROM pg_extension ORDER BY extname")
    members = rows("""
        SELECT e.extname AS extension, pg_describe_object(d.classid,d.objid,d.objsubid) AS object
        FROM pg_depend d JOIN pg_extension e ON e.oid=d.refobjid
        WHERE d.refclassid='pg_extension'::regclass AND d.deptype='e'
        ORDER BY e.extname, object
    """)
    reference_rows = {}
    for name in ("products", "delivery_points"):
        fields = ', '.join(f'"{column["name"]}"::text AS "{column["name"]}"' for column in tables[name]["columns"])
        reference_rows[name] = rows(f'SELECT {fields} FROM public."{name}" ORDER BY id')
    return CatalogManifest(
        revision=connection.execute(text("SELECT version_num FROM alembic_version")).scalar_one(),
        tables=tables, sequences=sequences, extensions=extensions,
        extension_members=members, reference_rows=reference_rows,
    )
