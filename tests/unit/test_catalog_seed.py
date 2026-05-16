"""Unit tests for the green-fuels catalog seed contract."""

from app.seeds.catalog_seed import DELIVERY_POINT_IDS, DELIVERY_POINTS, PRODUCT_IDS, PRODUCTS
from app.seeds.market_seed import CI_DATA, PRICING
from app.routers.catalog import DELIVERY_POINT_DISPLAY_ORDER


EXPECTED_PRODUCT_NAMES = {
    "Bio Methanol",
    "e-Methanol",
    "Bio Ethanol",
    "Synthetic Ethanol",
}

EXPECTED_DELIVERY_POINT_NAMES = {
    "Dalian",
    "Busan",
    "Shanghai",
    "Singapore",
    "Rotterdam",
    "Houston",
    "Los Angeles",
    "Santos",
}

EXPECTED_DELIVERY_POINT_ORDER = [
    "Dalian",
    "Busan",
    "Shanghai",
    "Singapore",
    "Rotterdam",
    "Houston",
    "Los Angeles",
    "Santos",
]


def test_catalog_seed_only_contains_approved_market_products():
    assert {product.name for product in PRODUCTS} == EXPECTED_PRODUCT_NAMES


def test_catalog_seed_exposes_deterministic_ids_for_approved_products_only():
    assert set(PRODUCT_IDS.keys()) == EXPECTED_PRODUCT_NAMES


def test_catalog_seed_only_contains_approved_delivery_points():
    assert {delivery_point.name for delivery_point in DELIVERY_POINTS} == EXPECTED_DELIVERY_POINT_NAMES


def test_catalog_api_display_order_matches_approved_delivery_point_sequence():
    assert list(DELIVERY_POINT_DISPLAY_ORDER.keys()) == EXPECTED_DELIVERY_POINT_ORDER


def test_catalog_seed_exposes_deterministic_ids_for_approved_delivery_points_only():
    assert set(DELIVERY_POINT_IDS.keys()) == EXPECTED_DELIVERY_POINT_NAMES


def test_market_seed_pricing_only_references_catalog_products():
    assert set(PRICING.keys()) == EXPECTED_PRODUCT_NAMES


def test_market_seed_pricing_only_references_catalog_delivery_points():
    for pricing_by_port in PRICING.values():
        assert set(pricing_by_port.keys()) == EXPECTED_DELIVERY_POINT_NAMES


def test_market_seed_ci_only_references_catalog_products():
    assert set(CI_DATA.keys()) == EXPECTED_PRODUCT_NAMES
