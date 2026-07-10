"""Tests for the compliance-adjusted pricing overlay service + endpoint (H1.2).

Golden hand-derivation (bio-methanol, CI 31 gCO2e/MJ, LCV 19.9 MJ/kg):

    CB improvement per MT  = (91.16 - 31) * 19.9 * 1000
                           = 60.16 * 19.9 * 1000 = 1,197,184 gCO2e
    marginal penalty rate  = 2400 EUR / (91.16 * 41000)
                           = 2400 / 3,737,560 EUR per gram of CB
    penalty avoided EUR/MT = 1,197,184 * 2400 / 3,737,560
                           = 2,873,241,600 / 3,737,560 = 768.74795... -> 768.75
    penalty avoided USD/MT = 768.74795... * 1.08 = 830.2478...       -> 830.25
    tCO2e avoided per MT   = 1,197,184 / 1,000,000 = 1.197184        -> 1.197
"""
import inspect
import json
from contextlib import asynccontextmanager
from datetime import UTC, datetime
from decimal import Decimal
from pathlib import Path
from typing import AsyncIterator
from uuid import uuid4

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient
from sqlalchemy import delete, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.config import settings
from app.database import Base, get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.port import Vessel
from app.models.user import OrgType, Organization, User, UserRole, UserStatus
from app.routers.auth_simple import get_current_user
from app.routers.compliance_api import router as compliance_api_router
from app.schemas.compliance_pricing import PricingOverlayResponse
from app.seeds.market_seed import DEMO_SELLER_ORG_ID
from app.services.demo_market import is_demo_market_organization
from app.services.compliance_pricing import (
    EUR_USD_RATE,
    compute_listing_overlay,
    fueleu_year_target,
    overlay_assumptions,
)


# ============== Pure service: penalty avoided / tCO2e avoided ==============


def test_golden_bio_methanol_product_default():
    # See module docstring for the full hand-derivation: 768.75 EUR/MT,
    # 830.25 USD/MT at EUR/USD 1.08, 1.197 tCO2e avoided/MT.
    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL",
        listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=None,
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("768.75")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("830.25")
    assert overlay.tco2e_avoided_per_mt == Decimal("1.197")
    assert overlay.ci_gco2_mj == Decimal("31")
    assert overlay.ci_basis == "PRODUCT_DEFAULT"
    assert overlay.lcv_mj_kg == Decimal("19.9")
    assert overlay.lcv_basis == "PRODUCT_DEFAULT"


def test_golden_bio_methanol_listing_ci():
    # Same golden numbers when the listing itself declares CI 31: the value
    # matches the product default but the basis must say LISTING.
    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL",
        listing_ci_gco2_mj=Decimal("31"),
        listing_lcv_mj_kg=None,
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("768.75")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("830.25")
    assert overlay.tco2e_avoided_per_mt == Decimal("1.197")
    assert overlay.ci_basis == "LISTING"
    assert overlay.lcv_basis == "PRODUCT_DEFAULT"


def test_listing_ci_and_lcv_override_product_defaults():
    # CI 20, LCV 21: (91.16 - 20) * 21 * 1000 = 1,494,360 g
    # EUR = 1,494,360 * 2400 / 3,737,560 = 3,586,464,000 / 3,737,560
    #     = 959.5735... -> 959.57;  USD = 959.5735 * 1.08 = 1036.339 -> 1036.34
    # tCO2e = 1.49436 -> 1.494
    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL",
        listing_ci_gco2_mj=Decimal("20"),
        listing_lcv_mj_kg=Decimal("21"),
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("959.57")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("1036.34")
    assert overlay.tco2e_avoided_per_mt == Decimal("1.494")
    assert overlay.ci_gco2_mj == Decimal("20")
    assert overlay.ci_basis == "LISTING"
    assert overlay.lcv_mj_kg == Decimal("21")
    assert overlay.lcv_basis == "LISTING"


def test_e_methanol_product_default():
    # (91.16 - 8) * 19.9 * 1000 = 1,654,884 g
    # EUR = 1,654,884 * 2400 / 3,737,560 = 3,971,721,600 / 3,737,560
    #     = 1062.65093... -> 1062.65;  USD = 1062.65093 * 1.08 -> 1147.66
    # tCO2e = 1.654884 -> 1.655
    overlay = compute_listing_overlay(
        market_product="E_METHANOL",
        listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=None,
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("1062.65")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("1147.66")
    assert overlay.tco2e_avoided_per_mt == Decimal("1.655")
    assert overlay.ci_gco2_mj == Decimal("8")
    assert overlay.lcv_mj_kg == Decimal("19.9")


def test_bio_ethanol_uses_class_proxy_defaults():
    # Biofuel-class proxy CI 35, ethanol-family LCV 26.8:
    # (91.16 - 35) * 26.8 * 1000 = 1,505,088 g
    # EUR = 1,505,088 * 2400 / 3,737,560 = 3,612,211,200 / 3,737,560
    #     = 966.4624... -> 966.46;  USD = 966.4624 * 1.08 = 1043.779 -> 1043.78
    # tCO2e = 1.505088 -> 1.505
    overlay = compute_listing_overlay(
        market_product="BIO_ETHANOL",
        listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=None,
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("966.46")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("1043.78")
    assert overlay.tco2e_avoided_per_mt == Decimal("1.505")


def test_synthetic_ethanol_not_zeroed_by_unknown_fuel_fallback():
    # e-fuel-class proxy CI 10: (91.16 - 10) * 26.8 * 1000 = 2,175,088 g
    # EUR = 2,175,088 * 2400 / 3,737,560 = 1396.6896... -> 1396.69
    # Routing through FUEL_GHG_INTENSITIES.get(fuel, 91.16) would have
    # silently produced 0.00 here.
    overlay = compute_listing_overlay(
        market_product="SYNTHETIC_ETHANOL",
        listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=None,
    )

    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("1396.69")
    assert overlay.tco2e_avoided_per_mt == Decimal("2.175")


def test_zero_floor_when_ci_at_or_above_baseline():
    for ci in (Decimal("91.16"), Decimal("95")):
        overlay = compute_listing_overlay(
            market_product="BIO_METHANOL",
            listing_ci_gco2_mj=ci,
            listing_lcv_mj_kg=None,
        )
        assert overlay is not None
        assert overlay.penalty_avoided_eur_per_mt == Decimal("0.00")
        assert overlay.penalty_avoided_usd_per_mt == Decimal("0.00")
        assert overlay.tco2e_avoided_per_mt == Decimal("0.000")


def test_unresolvable_rows_return_none():
    # Unknown market product and no listing data: neither CI nor LCV resolves.
    assert (
        compute_listing_overlay(
            market_product=None,
            listing_ci_gco2_mj=None,
            listing_lcv_mj_kg=None,
        )
        is None
    )
    # Listing CI alone does not help when the LCV has no product default.
    assert (
        compute_listing_overlay(
            market_product=None,
            listing_ci_gco2_mj=Decimal("31"),
            listing_lcv_mj_kg=None,
        )
        is None
    )
    # Full listing data prices fine even without a recognized market product.
    overlay = compute_listing_overlay(
        market_product=None,
        listing_ci_gco2_mj=Decimal("31"),
        listing_lcv_mj_kg=Decimal("19.9"),
    )
    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("768.75")
    assert overlay.ci_basis == "LISTING"
    assert overlay.lcv_basis == "LISTING"


def test_service_never_references_fuel_ghg_intensities():
    # FUEL_GHG_INTENSITIES has no Ethanol entry and its .get(fuel, 91.16)
    # fallback silently zeroes the advantage of unknown fuels; the overlay
    # service must resolve CI via PRODUCT_DEFAULT_CI instead.
    import app.services.compliance_pricing as module

    source = Path(inspect.getsourcefile(module)).read_text()
    assert "FUEL_GHG_INTENSITIES" not in source

    from app.services.compliance_scoring import FUEL_GHG_INTENSITIES

    assert "Ethanol" not in FUEL_GHG_INTENSITIES  # the trap this guards against


# ============== Assumptions object ==============


def test_year_target_step_function():
    # Explicit steps (2025-2029 -> 89.34, 2030-2034 -> 80.04,
    # 2035-2049 -> 65.08, 2050+ -> 9.12); FUELEU_TARGETS.get(year, default)
    # would mis-report 2031+ as 89.34.
    assert fueleu_year_target(2025) == Decimal("89.34")
    assert fueleu_year_target(2029) == Decimal("89.34")
    assert fueleu_year_target(2030) == Decimal("80.04")
    assert fueleu_year_target(2031) == Decimal("80.04")
    assert fueleu_year_target(2034) == Decimal("80.04")
    assert fueleu_year_target(2035) == Decimal("65.08")
    assert fueleu_year_target(2036) == Decimal("65.08")
    assert fueleu_year_target(2049) == Decimal("65.08")
    assert fueleu_year_target(2050) == Decimal("9.12")
    assert fueleu_year_target(2060) == Decimal("9.12")


def test_assumptions_contents_default_fleet():
    assumptions = overlay_assumptions(year=2026, fleet_vessel_count=0)

    assert assumptions.eur_usd_rate == Decimal("1.08")
    assert assumptions.vlsfo_baseline_gco2_mj == Decimal("91.16")
    assert assumptions.ghgie_actual_gco2_mj == Decimal("91.16")
    assert assumptions.fleet_intensity_basis == "DEFAULT_VLSFO"
    assert assumptions.fleet_vessel_count == 0
    assert assumptions.penalty_eur_per_tonne == Decimal("2400")
    assert assumptions.year == 2026
    assert assumptions.year_target == Decimal("89.34")
    assert assumptions.excluded_factors == [
        "RFNBO_MULTIPLIER",
        "DEFICIT_ESCALATION",
        "EXTRA_EU_VOYAGE_SCOPE",
    ]


def test_assumptions_org_fleet_basis_keeps_prototype_ghgie():
    # Vessels present flips the basis label only; the prototype still
    # computes with GHGIE_actual = 91.16 (H1.1 changes the number, not the
    # API shape).
    assumptions = overlay_assumptions(year=2031, fleet_vessel_count=3)

    assert assumptions.fleet_intensity_basis == "ORG_FLEET"
    assert assumptions.fleet_vessel_count == 3
    assert assumptions.ghgie_actual_gco2_mj == Decimal("91.16")
    assert assumptions.year_target == Decimal("80.04")


def test_eur_usd_rate_settings_override(monkeypatch):
    # 768.74795... * 1.10 = 845.6227... -> 845.62
    monkeypatch.setattr(settings, "COMPLIANCE_EUR_USD_RATE", Decimal("1.10"))

    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL",
        listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=None,
    )
    assert overlay is not None
    assert overlay.penalty_avoided_eur_per_mt == Decimal("768.75")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("845.62")

    assumptions = overlay_assumptions(year=2026, fleet_vessel_count=0)
    assert assumptions.eur_usd_rate == Decimal("1.10")

    # Module constant stays the ASSUMED default, untouched by the override.
    assert EUR_USD_RATE == Decimal("1.08")


def test_json_emits_decimals_as_strings():
    # The fe coerces with Number(); pin the string contract so a pydantic
    # config change cannot silently switch to JSON floats.
    priced_id = uuid4()
    response = PricingOverlayResponse(
        overlays={
            priced_id: compute_listing_overlay(
                market_product="BIO_METHANOL",
                listing_ci_gco2_mj=None,
                listing_lcv_mj_kg=None,
            ),
            uuid4(): None,
        },
        assumptions=overlay_assumptions(year=2026, fleet_vessel_count=0),
    )

    payload = json.loads(response.model_dump_json())
    row = payload["overlays"][str(priced_id)]
    assert row["penalty_avoided_eur_per_mt"] == "768.75"
    assert row["penalty_avoided_usd_per_mt"] == "830.25"
    assert row["tco2e_avoided_per_mt"] == "1.197"
    assert payload["assumptions"]["eur_usd_rate"] == "1.08"
    assert payload["assumptions"]["vlsfo_baseline_gco2_mj"] == "91.16"
    assert payload["assumptions"]["penalty_eur_per_tonne"] == "2400"
    nulled = [value for value in payload["overlays"].values() if value is None]
    assert len(nulled) == 1


# ============== Endpoint: POST /api/compliance/pricing-overlay ==============


REQUIRED_TABLES = [
    "organizations",
    "users",
    "products",
    "delivery_points",
    "orderbook_orders",
]

# vessels carries PostGIS Geography columns whose DDL sqlite cannot parse;
# the endpoint only counts rows, so a stand-in with the queried columns
# (plus the ones an ORM insert populates) is enough.
VESSELS_STANDIN_DDL = (
    "CREATE TABLE vessels ("
    "id UUID PRIMARY KEY, organization_id UUID, "
    "name VARCHAR, imo_number VARCHAR, updated_at DATETIME)"
)


@pytest.fixture(scope="module")
def async_engine():
    return create_async_engine("sqlite+aiosqlite://", echo=False, future=True)


@pytest.fixture(scope="module")
async def setup_tables(async_engine):
    tables = [Base.metadata.tables[name] for name in REQUIRED_TABLES]
    async with async_engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=tables)
        await conn.execute(text(VESSELS_STANDIN_DDL))
    yield
    async with async_engine.begin() as conn:
        await conn.execute(text("DROP TABLE vessels"))
        await conn.run_sync(Base.metadata.drop_all, tables=tables)


@pytest.fixture
async def db(async_engine, setup_tables):
    session_factory = async_sessionmaker(
        bind=async_engine, class_=AsyncSession, expire_on_commit=False, autoflush=False
    )
    async with session_factory() as session:
        yield session
        await session.rollback()
        await session.execute(text("DELETE FROM vessels"))
        for table in ("orderbook_orders", "users", "delivery_points", "products", "organizations"):
            await session.execute(delete(Base.metadata.tables[table]))
        await session.commit()


@asynccontextmanager
async def overlay_client(db_session: AsyncSession, user: User | None = None) -> AsyncIterator[AsyncClient]:
    app = FastAPI()

    async def override_get_db():
        yield db_session

    app.dependency_overrides[get_db] = override_get_db

    if user is not None:

        async def override_current_user():
            return user

        app.dependency_overrides[get_current_user] = override_current_user

    app.include_router(compliance_api_router, prefix="/api")

    async with AsyncClient(transport=ASGITransport(app=app), base_url="https://test") as client:
        yield client


async def _make_org(db: AsyncSession, name: str, org_type: OrgType, *, org_id=None) -> Organization:
    org = Organization(name=f"{name}-{uuid4().hex[:6]}", type=org_type)
    if org_id is not None:
        org.id = org_id
    db.add(org)
    await db.flush()
    return org


async def _make_user(db: AsyncSession, org: Organization, role: UserRole) -> User:
    user = User(
        email=f"{uuid4().hex[:8]}@example.com",
        password_hash="hashed",
        role=role,
        status=UserStatus.APPROVED,
        organization_id=org.id,
    )
    db.add(user)
    await db.flush()
    return user


async def _make_product(db: AsyncSession, *, name: str, fuel_type: str, fuel_grade: str) -> Product:
    product = Product(name=f"{name}-{uuid4().hex[:6]}", fuel_type=fuel_type, fuel_grade=fuel_grade)
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str) -> DeliveryPoint:
    delivery_point = DeliveryPoint(name=f"{name}-{uuid4().hex[:6]}", region="Asia")
    db.add(delivery_point)
    await db.flush()
    return delivery_point


async def _make_order(
    db: AsyncSession,
    *,
    organization_id,
    product_id,
    delivery_point_id,
    side: OrderSide = OrderSide.ASK,
    status: OrderBookStatus = OrderBookStatus.OPEN,
    ci: str | None = "31",
    lcv: str | None = None,
    off_spec: bool = False,
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=organization_id,
        side=side,
        product_id=product_id,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal("1200"),
        availability_window="SPOT",
        status=status,
        created_at=datetime.now(UTC),
        certification_declared=(side == OrderSide.ASK),
        certification_scheme="ISCC EU",
        specification_standard="IMPCA",
        msds_available=True,
        certifications=["ISCC EU"],
        carbon_intensity_gco2_mj=Decimal(ci) if ci is not None else None,
        energy_density_mj_kg=Decimal(lcv) if lcv is not None else None,
        feedstock="waste wood",
        origin="Netherlands",
        off_spec=off_spec,
    )
    db.add(order)
    await db.flush()
    return order


async def _marketplace_fixture(db: AsyncSession) -> tuple[User, Organization, Product, DeliveryPoint]:
    buyer_org = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, "Supplier", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    singapore = await _make_delivery_point(db, "Singapore")
    product = await _make_product(db, name="Bio Methanol", fuel_type="Methanol", fuel_grade="Bio")
    return buyer, supplier_org, product, singapore


async def test_endpoint_requires_authentication(db: AsyncSession):
    async with overlay_client(db) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(uuid4())]},
        )
    assert response.status_code == 401


async def test_endpoint_rejects_more_than_100_ids(db: AsyncSession):
    buyer, _, _, _ = await _marketplace_fixture(db)
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(uuid4()) for _ in range(101)]},
        )
    assert response.status_code == 422


async def test_endpoint_rejects_unknown_fields(db: AsyncSession):
    buyer, _, _, _ = await _marketplace_fixture(db)
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(uuid4())], "unexpected": True},
        )
    assert response.status_code == 422


async def test_endpoint_prices_visible_ask_with_golden_values(db: AsyncSession):
    buyer, supplier_org, product, singapore = await _marketplace_fixture(db)
    ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        ci="31",
    )
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(ask.id)]},
        )

    assert response.status_code == 200
    payload = response.json()
    row = payload["overlays"][str(ask.id)]
    # Decimal-as-string over the wire (fe coerces with Number()).
    assert row["penalty_avoided_eur_per_mt"] == "768.75"
    assert row["penalty_avoided_usd_per_mt"] == "830.25"
    assert row["tco2e_avoided_per_mt"] == "1.197"
    assert Decimal(row["ci_gco2_mj"]) == Decimal("31")
    assert row["ci_basis"] == "LISTING"
    assert Decimal(row["lcv_mj_kg"]) == Decimal("19.9")
    assert row["lcv_basis"] == "PRODUCT_DEFAULT"

    assumptions = payload["assumptions"]
    assert assumptions["eur_usd_rate"] == "1.08"
    assert assumptions["fleet_intensity_basis"] == "DEFAULT_VLSFO"
    assert assumptions["fleet_vessel_count"] == 0
    assert assumptions["year"] == 2026
    assert assumptions["year_target"] == "89.34"
    assert assumptions["excluded_factors"] == [
        "RFNBO_MULTIPLIER",
        "DEFICIT_ESCALATION",
        "EXTRA_EU_VOYAGE_SCOPE",
    ]


async def test_endpoint_non_visible_ids_are_null_indistinguishably(db: AsyncSession):
    buyer, supplier_org, product, singapore = await _marketplace_fixture(db)
    visible_ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
    )
    bid = await _make_order(
        db,
        organization_id=buyer.organization_id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        side=OrderSide.BID,
        ci=None,
    )
    cancelled_ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        status=OrderBookStatus.CANCELLED,
    )
    off_spec_ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
        off_spec=True,
    )
    unknown_id = str(uuid4())
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={
                "order_ids": [
                    str(visible_ask.id),
                    str(bid.id),
                    str(cancelled_ask.id),
                    str(off_spec_ask.id),
                    unknown_id,
                ]
            },
        )

    assert response.status_code == 200
    overlays = response.json()["overlays"]
    assert overlays[str(visible_ask.id)] is not None
    # No existence oracle: bid, cancelled, off-spec and nonexistent ids all
    # come back as the same null.
    assert overlays[str(bid.id)] is None
    assert overlays[str(cancelled_ask.id)] is None
    assert overlays[str(off_spec_ask.id)] is None
    assert overlays[unknown_id] is None


async def test_endpoint_demo_ask_is_priced(db: AsyncSession):
    buyer, _, product, singapore = await _marketplace_fixture(db)
    demo_org = await _make_org(db, "Demo", OrgType.FUEL_SUPPLIER, org_id=DEMO_SELLER_ORG_ID)
    assert is_demo_market_organization(demo_org.id)
    demo_ask = await _make_order(
        db,
        organization_id=demo_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
    )
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(demo_ask.id)]},
        )

    assert response.status_code == 200
    row = response.json()["overlays"][str(demo_ask.id)]
    assert row is not None
    assert row["penalty_avoided_usd_per_mt"] == "830.25"


async def test_endpoint_org_fleet_basis_when_org_has_vessels(db: AsyncSession):
    buyer, supplier_org, product, singapore = await _marketplace_fixture(db)
    ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
    )
    db.add(Vessel(organization_id=buyer.organization_id, name="MV Test", imo_number=f"IMO{uuid4().hex[:7]}"))
    await db.flush()
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(ask.id)], "year": 2031},
        )

    assert response.status_code == 200
    payload = response.json()
    assumptions = payload["assumptions"]
    assert assumptions["fleet_intensity_basis"] == "ORG_FLEET"
    assert assumptions["fleet_vessel_count"] == 1
    # Prototype-honest: the basis label changes, the number does not.
    assert assumptions["ghgie_actual_gco2_mj"] == "91.16"
    assert assumptions["year"] == 2031
    assert assumptions["year_target"] == "80.04"
    # year is annotation-only: the marginal math has no year term.
    assert payload["overlays"][str(ask.id)]["penalty_avoided_usd_per_mt"] == "830.25"
