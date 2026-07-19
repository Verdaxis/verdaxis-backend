"""Explicit Alembic comparison exclusions for owned legacy/system objects."""

from __future__ import annotations

import re
from typing import Any
from sqlalchemy import Enum, String


# The PostGIS image's tiger/geocoder catalog is installed in the public
# database alongside spatial_ref_sys. These are extension-owned, not
# application-owned, and are intentionally enumerated rather than ignored by
# object type.
POSTGIS_SYSTEM_TABLES = {
    "addr",
    "addrfeat",
    "bg",
    "county",
    "county_lookup",
    "countysub_lookup",
    "cousub",
    "direction_lookup",
    "edges",
    "faces",
    "featnames",
    "geocode_settings",
    "geocode_settings_default",
    "layer",
    "loader_lookuptables",
    "loader_platform",
    "loader_variables",
    "pagc_gaz",
    "pagc_lex",
    "pagc_rules",
    "place",
    "place_lookup",
    "secondary_unit_lookup",
    "spatial_ref_sys",
    "state",
    "state_lookup",
    "street_type_lookup",
    "tabblock",
    "tabblock20",
    "topology",
    "tract",
    "zcta5",
    "zip_lookup",
    "zip_lookup_all",
    "zip_lookup_base",
    "zip_state",
    "zip_state_loc",
}

# These three columns and the two named RFQ objects are present in the live
# schemas but are owned by the security/market integration branches. They are
# excluded only by exact table/name/type/default fingerprints until the
# combined integration migration adopts them; this is not a category-wide
# reflected-object suppression.
EXTERNAL_SCHEMA_FINGERPRINTS = {
    ("rfqs", "reference_number", "VARCHAR(20)", True, None),
    ("rfqs", "daily_seq", "INTEGER", True, None),
    ("rfq_quotes", "last_counter_by", "VARCHAR(10)", True, "NULL"),
    ("rfqs", "ix_rfqs_reference_number", "index", None, None),
    ("rfqs", "reference_number_key", "unique_constraint", None, None),
}

# Historical Verdaxis migrations retain these tables for data moves, but the
# current ORM deliberately does not own them. Any future exclusion must name
# its object here and be documented as legacy.
LEGACY_TABLES = {
    "contracts",
    "demand_profiles",
    "direct_order_offers",
    "direct_orders",
    "orders",
    "public_listings",
    "supply_listings",
}


def include_object(
    object_: Any,
    name: str,
    type_: str,
    reflected: bool,
    compare_to: Any,
) -> bool:
    """Compare everything except explicitly documented non-application objects."""
    if type_ == "table" and name in POSTGIS_SYSTEM_TABLES | LEGACY_TABLES:
        return False

    table = getattr(object_, "table", None)
    table_name = getattr(table, "name", None)
    if table_name in POSTGIS_SYSTEM_TABLES | LEGACY_TABLES:
        return False

    if type_ == "foreign_key_constraint":
        referred_table = getattr(object_, "referred_table", None)
        if getattr(referred_table, "name", None) in LEGACY_TABLES:
            return False

    if reflected and compare_to is None and table_name in {"rfqs", "rfq_quotes"}:
        if type_ == "column":
            type_name = str(getattr(object_, "type", "")).upper().replace(" ", "")
            nullable = getattr(object_, "nullable", None)
            default = _normalize_server_default(getattr(object_, "server_default", None))
            fingerprint = (table_name, name, type_name, nullable, default)
            if fingerprint in EXTERNAL_SCHEMA_FINGERPRINTS:
                return False
        elif type_ == "index" and (table_name, name, "index", None, None) in EXTERNAL_SCHEMA_FINGERPRINTS:
            return False
        elif type_ == "unique_constraint" and (table_name, name, "unique_constraint", None, None) in EXTERNAL_SCHEMA_FINGERPRINTS:
            return False
    # SQLAlchemy does not consistently attach the reflected table to a
    # PostgreSQL unique constraint during autogenerate. Keep this one exact
    # integration-owned object allowlisted by name rather than suppressing
    # every reflected unique constraint.
    if (
        reflected
        and compare_to is None
        and type_ == "unique_constraint"
        and name in {"reference_number_key", "rfqs_reference_number_key"}
    ):
        return False

    return True


def compare_type(context: Any, inspected_column: Any, metadata_column: Any,
                 inspected_type: Any, metadata_type: Any) -> bool | None:
    """Delegate enum/string comparisons to Alembic's exact type comparator.

    Non-native SQLAlchemy enums are intentionally stored as VARCHAR values.
    The callback only returns a result for this precise Enum-to-VARCHAR
    representation. A changed length or reflected value constraint is a real
    diff; unrelated types use Alembic's normal comparison implementation.
    """
    if isinstance(metadata_type, Enum) and isinstance(inspected_type, String):
        if inspected_type.length != metadata_type.length:
            return True
        expected_values = tuple(str(value) for value in metadata_type.enums)
        actual_values = _reflected_enum_values(inspected_column)
        if actual_values is not None and actual_values != expected_values:
            return True
        return False
    return None


def _reflected_enum_values(inspected_column: Any) -> tuple[str, ...] | None:
    """Read a simple reflected ``CHECK (column IN (...))`` value set, if present."""
    constraints = set(getattr(inspected_column, "constraints", ()) or ())
    table = getattr(inspected_column, "table", None)
    constraints.update(getattr(table, "constraints", ()) or ())
    column_name = getattr(inspected_column, "name", "")
    patterns = (
        re.compile(
            rf"\b{re.escape(column_name)}\b\s+IN\s*\((?P<values>[^)]*)\)",
            flags=re.IGNORECASE,
        ),
        re.compile(
            rf"\b{re.escape(column_name)}\b.*?ARRAY\s*\[(?P<values>[^]]*)\]",
            flags=re.IGNORECASE,
        ),
    )
    for constraint in constraints:
        sqltext = str(getattr(constraint, "sqltext", ""))
        for pattern in patterns:
            match = pattern.search(sqltext)
            if match:
                return tuple(
                    value.replace("''", "'")
                    for value in re.findall(r"'((?:''|[^'])*)'", match.group("values"))
                )
    return None


def _normalize_server_default(value: Any) -> str | None:
    """Normalize equivalent PostgreSQL default expressions without erasing values."""
    if value is None:
        return None
    value = getattr(value, "arg", value)
    expression = re.sub(r"\s+", " ", str(value).strip())
    while expression.startswith("(") and expression.endswith(")"):
        inner = expression[1:-1].strip()
        if inner.count("(") != inner.count(")"):
            break
        expression = inner
    expression = re.sub(
        r"::(?:timestamp with time zone|character varying|double precision|varchar|text|boolean|json|jsonb|numeric|integer|timestamp)",
        "",
        expression,
        flags=re.IGNORECASE,
    )
    expression = re.sub(r"\bCURRENT_TIMESTAMP\b", "now()", expression, flags=re.IGNORECASE)
    expression = re.sub(r"\bNOW\s*\(\s*\)", "now()", expression, flags=re.IGNORECASE)
    expression = re.sub(r"\b(TRUE|FALSE)\b", lambda match: match.group(1).lower(), expression, flags=re.IGNORECASE)
    if re.fullmatch(r"'[-+]?\d+(?:\.\d+)?'", expression):
        expression = expression[1:-1]
    return expression


def compare_server_default(
    context: Any,
    inspected_column: Any,
    metadata_column: Any,
    inspected_default: Any,
    metadata_default: Any,
    rendered_metadata_default: Any,
) -> bool | None:
    """Compare actual expressions and only ignore an absent Python-only default.

    A Python default is not permission to suppress a database default: if the
    database has an expression where the model does not, Alembic must report
    it. Explicit server defaults are considered equal only after a narrow
    expression normalization (casts, whitespace, and equivalent ``now()``).
    """
    actual = _normalize_server_default(inspected_default)
    expected = _normalize_server_default(rendered_metadata_default or metadata_default)
    if expected is None:
        if actual is None and getattr(metadata_column, "default", None) is not None:
            return False
        if (
            getattr(metadata_column, "default", None) is not None
            and _python_default_compatibility_fingerprint(metadata_column, actual)
        ):
            return False
        return None
    if actual is not None and actual == expected:
        return False
    return None


_PYTHON_DEFAULT_COMPATIBILITY = {
    ("benchmarks", "source", "'manual_override'"),
    ("benchmarks", "created_at", "now()"),
    ("benchmarks", "updated_at", "now()"),
    ("compliance_ledger", "id", "gen_random_uuid()"),
    ("compliance_ledger", "currency", "'EUR'"),
    ("compliance_ledger", "created_at", "now()"),
    ("delivery_points", "is_active", "true"),
    ("inventory_items", "id", "gen_random_uuid()"),
    ("inventory_items", "is_certified", "false"),
    ("inventory_items", "updated_at", "now()"),
    ("live_slice_benchmarks", "order_count", "0"),
    ("live_slice_benchmarks", "source", "'live_slice_vwap'"),
    ("live_slice_benchmarks", "created_at", "now()"),
    ("live_slice_benchmarks", "updated_at", "now()"),
    ("match_suggestions", "id", "gen_random_uuid()"),
    ("negotiation_rounds", "created_at", "now()"),
    ("negotiations", "status", "'OPEN'"),
    ("negotiations", "created_at", "now()"),
    ("negotiations", "updated_at", "now()"),
    ("news_items", "category", "'markets'"),
    ("news_items", "relevance", "3"),
    ("orderbook_orders", "id", "gen_random_uuid()"),
    ("organizations", "id", "gen_random_uuid()"),
    ("organizations", "verification_status", "'PENDING'"),
    ("organizations", "created_at", "now()"),
    ("port_intelligence", "id", "gen_random_uuid()"),
    ("port_intelligence", "captured_at", "now()"),
    ("ports", "is_active", "true"),
    ("price_alerts", "is_active", "true"),
    ("producer_projects", "id", "gen_random_uuid()"),
    ("products", "unit", "'MT'"),
    ("products", "min_lot_size", "100"),
    ("products", "is_active", "true"),
    ("referrals", "id", "gen_random_uuid()"),
    ("referrals", "status", "'SIGNED_UP'"),
    ("referrals", "created_at", "now()"),
    ("rfq_quotes", "status", "'PENDING'"),
    ("rfq_quotes", "created_at", "now()"),
    ("rfqs", "availability_window", "'SPOT'"),
    ("rfqs", "is_anonymous", "false"),
    ("rfqs", "status", "'OPEN'"),
    ("rfqs", "created_at", "now()"),
    ("subscriptions", "tier", "'free'"),
    ("subscriptions", "is_active", "true"),
    ("traceability_events", "id", "gen_random_uuid()"),
    ("traceability_events", "is_verified", "false"),
    ("trades", "id", "gen_random_uuid()"),
    ("users", "id", "gen_random_uuid()"),
    ("vessels", "id", "gen_random_uuid()"),
    ("vessels", "updated_at", "now()"),
    ("watchlist_entries", "created_at", "now()"),
    ("watchlist_events", "event_payload", "'{}'"),
    ("watchlist_events", "created_at", "now()"),
    ("watchlist_targets", "created_at", "now()"),
    ("watchlists", "created_at", "now()"),
}


def _python_default_compatibility_fingerprint(column: Any, actual: str | None) -> bool:
    table = getattr(getattr(column, "table", None), "name", None)
    return (table, getattr(column, "name", None), actual) in _PYTHON_DEFAULT_COMPATIBILITY
