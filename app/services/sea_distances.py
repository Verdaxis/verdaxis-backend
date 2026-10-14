"""Static sea distances between the Verdaxis delivery points.

The table is symmetric: one entry serves both directions. A pair has a
DEFAULT route, and also a CAPE_OF_GOOD_HOPE route when its default route
transits the Suez Canal.

SEA_DISTANCE_METHOD records how the numbers were produced. The planner
reports distances from this table as VERDAXIS_TABLE so callers can tell them
apart from their own figures.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal

from app.schemas.voyage import SeaRoute


@dataclass(frozen=True)
class SeaDistance:
    distance_nm: Decimal
    via: str  # for example "Suez Canal", "Cape of Good Hope", "Panama Canal", "Direct"


SEA_DISTANCE_METHOD = (
    "Open-source searoute 1.6.0 shortest sea paths on the default network, measured "
    "on 2026-10-14 and rounded to the nearest 10 nautical miles. Planning estimates "
    "only: not navigational distances, and not for passage planning."
)

# Keys are the two catalog port names in sorted order, then the route, so one
# entry serves both directions. Distances are nautical miles rounded to the
# nearest 10. CAPE_OF_GOOD_HOPE entries exist only for pairs whose DEFAULT route
# transits Suez; they close Suez, Bab-el-Mandeb and Panama so the path rounds the
# Cape instead of crossing another canal. "via" names the passage the path used.
_DISTANCES: dict[tuple[str, str, SeaRoute], SeaDistance] = {
    ("Busan", "Dalian", SeaRoute.DEFAULT): SeaDistance(Decimal("540"), "Direct"),
    ("Busan", "Houston", SeaRoute.DEFAULT): SeaDistance(Decimal("9740"), "Panama Canal"),
    ("Busan", "Los Angeles", SeaRoute.DEFAULT): SeaDistance(Decimal("5290"), "Direct"),
    ("Busan", "Rotterdam", SeaRoute.CAPE_OF_GOOD_HOPE): SeaDistance(Decimal("14230"), "Cape of Good Hope"),
    ("Busan", "Rotterdam", SeaRoute.DEFAULT): SeaDistance(Decimal("10910"), "Suez Canal"),
    ("Busan", "Santos", SeaRoute.DEFAULT): SeaDistance(Decimal("11420"), "Cape of Good Hope"),
    ("Busan", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("470"), "Direct"),
    ("Busan", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("2540"), "Direct"),
    ("Dalian", "Houston", SeaRoute.DEFAULT): SeaDistance(Decimal("10280"), "Panama Canal"),
    ("Dalian", "Los Angeles", SeaRoute.DEFAULT): SeaDistance(Decimal("5840"), "Direct"),
    ("Dalian", "Rotterdam", SeaRoute.CAPE_OF_GOOD_HOPE): SeaDistance(Decimal("14320"), "Cape of Good Hope"),
    ("Dalian", "Rotterdam", SeaRoute.DEFAULT): SeaDistance(Decimal("11000"), "Suez Canal"),
    ("Dalian", "Santos", SeaRoute.DEFAULT): SeaDistance(Decimal("11510"), "Cape of Good Hope"),
    ("Dalian", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("550"), "Direct"),
    ("Dalian", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("2620"), "Direct"),
    ("Houston", "Los Angeles", SeaRoute.DEFAULT): SeaDistance(Decimal("4520"), "Panama Canal"),
    ("Houston", "Rotterdam", SeaRoute.DEFAULT): SeaDistance(Decimal("5060"), "Direct"),
    ("Houston", "Santos", SeaRoute.DEFAULT): SeaDistance(Decimal("5620"), "Direct"),
    ("Houston", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("10210"), "Panama Canal"),
    ("Houston", "Singapore", SeaRoute.CAPE_OF_GOOD_HOPE): SeaDistance(Decimal("13160"), "Cape of Good Hope"),
    ("Houston", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("11770"), "Suez Canal"),
    ("Los Angeles", "Rotterdam", SeaRoute.DEFAULT): SeaDistance(Decimal("7830"), "Panama Canal"),
    ("Los Angeles", "Santos", SeaRoute.DEFAULT): SeaDistance(Decimal("7570"), "Panama Canal"),
    ("Los Angeles", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("5760"), "Direct"),
    ("Los Angeles", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("7790"), "Direct"),
    ("Rotterdam", "Santos", SeaRoute.DEFAULT): SeaDistance(Decimal("5500"), "Direct"),
    ("Rotterdam", "Shanghai", SeaRoute.CAPE_OF_GOOD_HOPE): SeaDistance(Decimal("13910"), "Cape of Good Hope"),
    ("Rotterdam", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("10590"), "Suez Canal"),
    ("Rotterdam", "Singapore", SeaRoute.CAPE_OF_GOOD_HOPE): SeaDistance(Decimal("11870"), "Cape of Good Hope"),
    ("Rotterdam", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("8380"), "Suez Canal"),
    ("Santos", "Shanghai", SeaRoute.DEFAULT): SeaDistance(Decimal("11100"), "Cape of Good Hope"),
    ("Santos", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("9060"), "Cape of Good Hope"),
    ("Shanghai", "Singapore", SeaRoute.DEFAULT): SeaDistance(Decimal("2210"), "Direct"),
}


def sea_distance(
    from_port: str,
    to_port: str,
    route: SeaRoute = SeaRoute.DEFAULT,
) -> SeaDistance | None:
    """Return the table distance for a pair of catalog ports, or None if the table has no entry."""
    first, second = sorted((from_port, to_port))
    return _DISTANCES.get((first, second, route))
