"""Unit tests for the green-fuels catalog seed contract."""

from uuid import UUID

from app.seeds.catalog_seed import DELIVERY_POINT_IDS, DELIVERY_POINTS, PRODUCT_IDS, PRODUCTS
from app.seeds.market_seed import CI_DATA, PRICING
from app.routers.catalog import DELIVERY_POINT_DISPLAY_ORDER


EXPECTED_ORDERBOOK_PRODUCT_NAMES = {
    "Bio Methanol",
    "e-Methanol",
    "Bio Ethanol",
    "Synthetic Ethanol",
}

EXPECTED_CATALOG_PRODUCT_NAMES = EXPECTED_ORDERBOOK_PRODUCT_NAMES | {"UCOME B100"}

EXPECTED_PRODUCT_IDS = {
    "UCOME B100": UUID("e561e43f-d9b2-598e-981c-f1d28d515ddc"),
    "Bio Methanol": UUID("9510c713-6e39-5080-add3-0a7c29657b79"),
    "e-Methanol": UUID("f9b20492-b445-59cd-b292-a386d913f488"),
    "Bio Ethanol": UUID("c4a688be-f7c2-5edc-8f93-6b34e387609c"),
    "Synthetic Ethanol": UUID("d186bffb-766d-5944-8825-989abbdcfc46"),
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
    assert {product.name for product in PRODUCTS} == EXPECTED_CATALOG_PRODUCT_NAMES
    assert {
        product.name for product in PRODUCTS if product.execution_mode == "ORDERBOOK"
    } == EXPECTED_ORDERBOOK_PRODUCT_NAMES
    ucome = next(product for product in PRODUCTS if product.name == "UCOME B100")
    assert ucome.execution_mode == "RFQ_ONLY"
    assert ucome.available_delivery_point_ids == (DELIVERY_POINT_IDS["Singapore"],)


def test_catalog_seed_exposes_deterministic_ids_for_approved_products_only():
    assert PRODUCT_IDS == EXPECTED_PRODUCT_IDS


def test_catalog_seed_only_contains_approved_delivery_points():
    assert {delivery_point.name for delivery_point in DELIVERY_POINTS} == EXPECTED_DELIVERY_POINT_NAMES


def test_catalog_api_display_order_matches_approved_delivery_point_sequence():
    assert list(DELIVERY_POINT_DISPLAY_ORDER.keys()) == EXPECTED_DELIVERY_POINT_ORDER


def test_catalog_seed_exposes_deterministic_ids_for_approved_delivery_points_only():
    assert set(DELIVERY_POINT_IDS.keys()) == EXPECTED_DELIVERY_POINT_NAMES


def test_market_seed_pricing_only_references_catalog_products():
    assert set(PRICING.keys()) == EXPECTED_ORDERBOOK_PRODUCT_NAMES


def test_market_seed_pricing_only_references_catalog_delivery_points():
    for pricing_by_port in PRICING.values():
        assert set(pricing_by_port.keys()) == EXPECTED_DELIVERY_POINT_NAMES


def test_market_seed_ci_only_references_catalog_products():
    assert set(CI_DATA.keys()) == EXPECTED_ORDERBOOK_PRODUCT_NAMES
