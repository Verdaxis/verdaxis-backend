"""Read PostgreSQL catalogs for the frozen Phase 0 copy contract.

This module opens no connections and grants no import authority. Callers own
connection attestation and transactions. Catalog inspection is read-only.
"""
from __future__ import annotations

import hashlib
import json
import re
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


# A single string literal cast to character varying, as PostgreSQL prints it
# inside a generated CHECK or index expression.
_VARCHAR_LITERAL = r"'(?:[^']|'')*'::character varying"
# One element of such an array, which may carry the round-trip text cast with
# or without the parentheses that the index printer adds.
_VARCHAR_ELEMENT = rf"(?:\({_VARCHAR_LITERAL}\)|{_VARCHAR_LITERAL})(?:::text)?"
# A complete ARRAY[...] of those elements, in either printed form. Form A casts
# the whole array and an index wraps it in parentheses; form B casts every
# element and drops both the parentheses and the array-wide cast.
_VARCHAR_ARRAY = re.compile(
    rf"\(ARRAY\[(?P<wrapped>{_VARCHAR_ELEMENT}(?:,\s*{_VARCHAR_ELEMENT})*)\]\)::text\[\]"
    rf"|ARRAY\[(?P<bare>{_VARCHAR_ELEMENT}(?:,\s*{_VARCHAR_ELEMENT})*)\](?:::text\[\])?"
)
# The per-element text cast of form B, with or without its parentheses.
_ELEMENT_TEXT_CAST = re.compile(rf"\(({_VARCHAR_LITERAL})\)::text|({_VARCHAR_LITERAL})::text")


def canonical_expression_text(text_value: Any) -> Any:
    """Normalize the two equal text forms of a ``varchar`` literal array.

    PostgreSQL renders ``col::text = ANY (...)`` over ``varchar`` constants in
    two equal forms. Which one appears depends on how the object was created.

    Fresh Alembic DDL writes form A, with a single ``::text[]`` cast on the
    whole array (and, in an index, parentheses around the array)::

        ARRAY['X'::character varying, 'Y'::character varying]::text[]
        (ARRAY['X'::character varying, 'Y'::character varying])::text[]

    A ``pg_dump``/``pg_restore`` round trip rewrites every such object into
    form B, with a ``::text`` cast on every element::

        ARRAY['X'::character varying::text, 'Y'::character varying::text]
        ARRAY[('X'::character varying)::text, ('Y'::character varying)::text]

    Both forms denote the same value, so a live database loaded from a dump and
    later migrated can hold both at once, and a restore turns every object into
    form B. This function rewrites either form to the plain
    ``ARRAY['X'::character varying, ...]`` core so that they compare equal. It
    matches nothing else: a non-string value and any text without such an array
    are returned unchanged, and a ``uuid`` or numeric array is never touched.
    """
    if not isinstance(text_value, str) or 'ARRAY[' not in text_value:
        return text_value

    def collapse(match):
        array = match.group('wrapped') if match.group('wrapped') is not None else match.group('bare')
        plain = _ELEMENT_TEXT_CAST.sub(lambda element: element.group(1) or element.group(2), array)
        return 'ARRAY[' + plain + ']'

    return _VARCHAR_ARRAY.sub(collapse, text_value)


def _keyed(rows, key, label):
    """Index an object list by its structural name, refusing a duplicate."""
    keyed = {}
    for row in rows:
        name = key(row)
        if name in keyed:
            raise ValueError(f'Duplicate catalog {label} name; import refused')
        keyed[name] = row
    return keyed


def _canonical_text(value):
    """Rewrite every string in a catalog structure to its canonical form."""
    if isinstance(value, dict):
        return {key: _canonical_text(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_canonical_text(item) for item in value]
    return canonical_expression_text(value)


def _normalized_table(table):
    table = dict(table)
    for field in ('columns', 'constraints', 'indexes', 'triggers'):
        table[field] = _keyed(table[field], lambda row: row['name'], field)
    return _canonical_text(table)


def _normalized_catalog(catalog):
    """Canonicalize a catalog for comparison only, keeping stored evidence raw.

    The comparison must not depend on collation or on physical attribute order.
    PostgreSQL sorts text with the database collation, so a live ``en_US.UTF-8``
    source and a ``C`` scratch cluster list the same extension members in a
    different order. Column attribute numbers also differ: the legacy source
    columns sit in the middle of ``rfqs`` in live but append to a fresh build.
    Neither property is one the copy depends on, so each object list is keyed by
    name and each expression is normalized before the two catalogs are compared.
    """
    result = {key: value for key, value in catalog.items() if key != 'reference_rows'}
    result['tables'] = {name: _normalized_table(table) for name, table in result['tables'].items()}
    result['sequences'] = _keyed(result['sequences'], lambda row: row['name'], 'sequence')
    result['extensions'] = _keyed(result['extensions'], lambda row: row['name'], 'extension')
    result['extension_members'] = _keyed(
        result['extension_members'],
        lambda row: (row['extension'], row['object']),
        'extension member',
    )
    return _canonical_text(result)


def require_complete_inventory(actual, declared) -> None:
    """Refuse extra/missing tables, columns, or changed structural definitions.

    Objects are compared by name and expressions by normalized text, so the
    result is independent of collation order and of attribute number. Reference
    values belong to the per-run data receipt, not the schema fingerprint:
    migration-generated created_at values vary by run.
    """
    if isinstance(actual, dict) and isinstance(declared, dict):
        actual = _normalized_catalog(actual)
        declared = _normalized_catalog(declared)
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
        ORDER BY n.nspname COLLATE "C", c.relname COLLATE "C"
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
            FROM pg_constraint WHERE conrelid=to_regclass(:table) ORDER BY conname COLLATE "C"
        """, table=qualified)
        indexes = rows("""
            SELECT c.relname AS name, pg_get_indexdef(i.indexrelid) AS definition,
                   i.indisvalid AS valid, i.indisready AS ready
            FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid
            WHERE i.indrelid=to_regclass(:table) ORDER BY c.relname COLLATE "C"
        """, table=qualified)
        triggers = rows("""
            SELECT t.tgname AS name, t.tgenabled AS enabled,
                   pg_get_triggerdef(t.oid, true) AS definition,
                   pg_get_functiondef(t.tgfoid) AS function
            FROM pg_trigger t WHERE t.tgrelid=to_regclass(:table) AND NOT t.tgisinternal
            ORDER BY t.tgname COLLATE "C"
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
        ORDER BY n.nspname COLLATE "C", c.relname COLLATE "C"
    """)
    extensions = rows('SELECT extname AS name, extversion AS version FROM pg_extension ORDER BY extname COLLATE "C"')
    members = rows("""
        SELECT e.extname AS extension, pg_describe_object(d.classid,d.objid,d.objsubid) AS object
        FROM pg_depend d JOIN pg_extension e ON e.oid=d.refobjid
        WHERE d.refclassid='pg_extension'::regclass AND d.deptype='e'
        ORDER BY e.extname COLLATE "C", pg_describe_object(d.classid,d.objid,d.objsubid) COLLATE "C"
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
