"""Explicit Alembic comparison exclusions for owned legacy/system objects."""

from __future__ import annotations

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

    return True


def compare_type(context: Any, inspected_column: Any, metadata_column: Any,
                 inspected_type: Any, metadata_type: Any) -> bool | None:
    """Treat Python enums stored in the documented VARCHAR representation as equal.

    The application intentionally uses ``native_enum=False``: the database
    stores validated string values, while ORM results remain Python enums.
    All other types use Alembic's normal comparison implementation.
    """
    if isinstance(metadata_type, Enum) and isinstance(inspected_type, String):
        return False
    return None


def compare_server_default(
    context: Any,
    inspected_column: Any,
    metadata_column: Any,
    inspected_default: Any,
    metadata_default: Any,
    rendered_metadata_default: Any,
) -> bool | None:
    """Keep DB defaults owned by Python-level defaults from becoming false drift.

    Explicit SQL defaults in metadata are still compared by Alembic. This
    only covers columns whose model has a Python default but intentionally no
    server default, which is the established ORM ownership pattern here.
    """
    if metadata_default is None and getattr(metadata_column, "default", None) is not None:
        return False
    return None
