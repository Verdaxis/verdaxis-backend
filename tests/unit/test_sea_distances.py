from decimal import Decimal
from itertools import combinations

import pytest

from app.market_catalog import CANONICAL_DELIVERY_POINTS
from app.schemas.voyage import SeaRoute
from app.services.sea_distances import SEA_DISTANCE_METHOD, sea_distance

PORTS = [spec.name for spec in CANONICAL_DELIVERY_POINTS]


def test_every_pair_has_a_default_route():
    for first, second in combinations(PORTS, 2):
        assert sea_distance(first, second) is not None, (first, second)


def test_lookup_is_symmetric():
    for first, second in combinations(PORTS, 2):
        assert sea_distance(first, second) == sea_distance(second, first)


def test_cape_route_exists_exactly_where_default_uses_suez():
    for first, second in combinations(PORTS, 2):
        default = sea_distance(first, second)
        cape = sea_distance(first, second, SeaRoute.CAPE_OF_GOOD_HOPE)
        if default.via == "Suez Canal":
            assert cape is not None and cape.distance_nm > default.distance_nm
        else:
            assert cape is None


@pytest.mark.parametrize(
    ("first", "second", "route", "low", "high"),
    [
        ("Shanghai", "Singapore", SeaRoute.DEFAULT, 2100, 2400),
        ("Singapore", "Rotterdam", SeaRoute.DEFAULT, 8100, 8600),
        ("Singapore", "Rotterdam", SeaRoute.CAPE_OF_GOOD_HOPE, 11400, 12000),
        ("Shanghai", "Rotterdam", SeaRoute.DEFAULT, 10300, 10900),
        ("Busan", "Dalian", SeaRoute.DEFAULT, 400, 700),
        ("Houston", "Rotterdam", SeaRoute.DEFAULT, 4800, 5300),
    ],
)
def test_known_distances_are_plausible(first, second, route, low, high):
    distance = sea_distance(first, second, route)
    assert distance is not None
    assert Decimal(low) <= distance.distance_nm <= Decimal(high)


def test_unknown_pair_returns_none():
    assert sea_distance("Singapore", "Atlantis") is None


def test_method_is_recorded():
    assert "searoute" in SEA_DISTANCE_METHOD and "2026-10-14" in SEA_DISTANCE_METHOD
