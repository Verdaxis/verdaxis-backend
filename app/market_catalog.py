"""Config-free identity contract for Verdaxis public markets.

The UUID and label tuples here are the runtime source of truth. Alembic
revisions intentionally carry frozen copies so old migrations remain
importable and deterministic after this module changes.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from enum import Enum
from types import MappingProxyType
from uuid import UUID


class MarketProduct(str, Enum):
    BIO_METHANOL = "BIO_METHANOL"
    E_METHANOL = "E_METHANOL"
    BIO_ETHANOL = "BIO_ETHANOL"
    SYNTHETIC_ETHANOL = "SYNTHETIC_ETHANOL"


@dataclass(frozen=True)
class ProductSpec:
    id: UUID
    market_product: MarketProduct
    name: str
    fuel_type: str
    fuel_grade: str
    unit: str
    min_lot_size: Decimal
    spec_description: str


@dataclass(frozen=True)
class DeliveryPointSpec:
    id: UUID
    name: str
    region: str
    timezone: str


CANONICAL_PRODUCTS: tuple[ProductSpec, ...] = (
    ProductSpec(
        UUID("9510c713-6e39-5080-add3-0a7c29657b79"),
        MarketProduct.BIO_METHANOL,
        "Bio Methanol",
        "Methanol",
        "Bio",
        "MT",
        Decimal("200"),
        "Bio-methanol produced from biogenic feedstocks for marine fuel use",
    ),
    ProductSpec(
        UUID("f9b20492-b445-59cd-b292-a386d913f488"),
        MarketProduct.E_METHANOL,
        "e-Methanol",
        "Methanol",
        "E",
        "MT",
        Decimal("200"),
        "Synthetic methanol produced from renewable hydrogen and captured CO2",
    ),
    ProductSpec(
        UUID("c4a688be-f7c2-5edc-8f93-6b34e387609c"),
        MarketProduct.BIO_ETHANOL,
        "Bio Ethanol",
        "Ethanol",
        "Bio",
        "MT",
        Decimal("200"),
        "Second-generation bioethanol from waste feedstocks",
    ),
    ProductSpec(
        UUID("d186bffb-766d-5944-8825-989abbdcfc46"),
        MarketProduct.SYNTHETIC_ETHANOL,
        "Synthetic Ethanol",
        "Ethanol",
        "Synthetic",
        "MT",
        Decimal("200"),
        "Synthetic ethanol produced via power-to-liquids or equivalent synthetic pathways",
    ),
)


# This tuple is also the public display order.
CANONICAL_DELIVERY_POINTS: tuple[DeliveryPointSpec, ...] = (
    DeliveryPointSpec(
        UUID("78281fb3-e726-5396-802e-34b01fda21e8"),
        "Dalian",
        "Asia",
        "Asia/Shanghai",
    ),
    DeliveryPointSpec(
        UUID("262d36ae-6f35-5785-b9e8-9e221b0f1b78"),
        "Busan",
        "Asia",
        "Asia/Seoul",
    ),
    DeliveryPointSpec(
        UUID("633c0593-f9b9-52ad-abe5-a0285f6888d3"),
        "Shanghai",
        "Asia",
        "Asia/Shanghai",
    ),
    DeliveryPointSpec(
        UUID("73835e92-820e-584b-8280-bb61c63aa28e"),
        "Singapore",
        "Asia",
        "Asia/Singapore",
    ),
    DeliveryPointSpec(
        UUID("1379d36c-1ca9-55b7-9c0d-5235a0ba1f36"),
        "Rotterdam",
        "Europe",
        "Europe/Amsterdam",
    ),
    DeliveryPointSpec(
        UUID("a083db06-b050-56c2-a274-3897eac2fdae"),
        "Houston",
        "Americas",
        "America/Chicago",
    ),
    DeliveryPointSpec(
        UUID("1be2a5cb-fc34-5e88-8b15-7f7d0fd6e534"),
        "Los Angeles",
        "Americas",
        "America/Los_Angeles",
    ),
    DeliveryPointSpec(
        UUID("a4762444-4e0b-5bc1-b647-a185047adae5"),
        "Santos",
        "Americas",
        "America/Sao_Paulo",
    ),
)


PRODUCTS_BY_CODE = MappingProxyType(
    {spec.market_product.value: spec for spec in CANONICAL_PRODUCTS}
)
PRODUCTS_BY_ID = MappingProxyType({spec.id: spec for spec in CANONICAL_PRODUCTS})
PRODUCTS_BY_NAME = MappingProxyType({spec.name: spec for spec in CANONICAL_PRODUCTS})
PRODUCT_IDS = MappingProxyType(
    {spec.market_product.value: spec.id for spec in CANONICAL_PRODUCTS}
)
DELIVERY_POINTS_BY_ID = MappingProxyType(
    {spec.id: spec for spec in CANONICAL_DELIVERY_POINTS}
)
DELIVERY_POINTS_BY_NAME = MappingProxyType(
    {spec.name: spec for spec in CANONICAL_DELIVERY_POINTS}
)
DELIVERY_POINT_IDS = MappingProxyType(
    {spec.name: spec.id for spec in CANONICAL_DELIVERY_POINTS}
)
MARKET_PRODUCT_CODES: tuple[str, ...] = tuple(PRODUCTS_BY_CODE)
CANONICAL_PRODUCT_IDS: tuple[UUID, ...] = tuple(PRODUCTS_BY_ID)
CANONICAL_DELIVERY_POINT_IDS: tuple[UUID, ...] = tuple(DELIVERY_POINTS_BY_ID)
DELIVERY_POINT_DISPLAY_ORDER = MappingProxyType(
    {spec.name: index for index, spec in enumerate(CANONICAL_DELIVERY_POINTS, start=1)}
)


def derive_market_product(
    name: str,
    fuel_type: str,
    fuel_grade: str,
) -> MarketProduct | None:
    """Resolve only an exact canonical label tuple; legacy aliases stay legacy."""
    identity = (name, fuel_type, fuel_grade)
    for spec in CANONICAL_PRODUCTS:
        if identity == (spec.name, spec.fuel_type, spec.fuel_grade):
            return spec.market_product
    return None
