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


SEA_DISTANCE_METHOD = "Not yet generated."

# Keys use the two catalog port names in sorted order, then the route.
_DISTANCES: dict[tuple[str, str, SeaRoute], SeaDistance] = {}


def sea_distance(
    from_port: str,
    to_port: str,
    route: SeaRoute = SeaRoute.DEFAULT,
) -> SeaDistance | None:
    """Return the table distance for a pair of catalog ports, or None if the table has no entry."""
    first, second = sorted((from_port, to_port))
    return _DISTANCES.get((first, second, route))
