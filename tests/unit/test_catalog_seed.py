"""Unit tests for the green-fuels catalog seed contract."""

from app.seeds.catalog_seed import PRODUCT_IDS, PRODUCTS
from app.seeds.market_seed import CI_DATA, PRICING


EXPECTED_PRODUCT_NAMES = {
    "Bio Methanol",
    "e-Methanol",
    "Bio Ethanol",
    "Synthetic Ethanol",
}


def test_catalog_seed_only_contains_approved_market_products():
    assert {product.name for product in PRODUCTS} == EXPECTED_PRODUCT_NAMES


def test_catalog_seed_exposes_deterministic_ids_for_approved_products_only():
    assert set(PRODUCT_IDS.keys()) == EXPECTED_PRODUCT_NAMES


def test_market_seed_pricing_only_references_catalog_products():
    assert set(PRICING.keys()) == EXPECTED_PRODUCT_NAMES


def test_market_seed_ci_only_references_catalog_products():
    assert set(CI_DATA.keys()) == EXPECTED_PRODUCT_NAMES
