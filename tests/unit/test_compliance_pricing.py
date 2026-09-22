"""Lifecycle comparisons must not invent consignment CI or cash benefits."""
import json
from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from decimal import Decimal
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
from app.market_catalog import DELIVERY_POINTS_BY_NAME, PRODUCTS_BY_NAME
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.user import (
    OrganizationProvenance,
    OrgType,
    Organization,
    User,
    UserRole,
    UserStatus,
)
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


@pytest.mark.parametrize("product", [
    "BIO_METHANOL", "E_METHANOL", "BIO_ETHANOL", "SYNTHETIC_ETHANOL", "UCOME_B100", None,
])
def test_unknown_ci_stays_unknown(product):
    assert compute_listing_overlay(
        market_product=product, listing_ci_gco2_mj=None,
        listing_lcv_mj_kg=Decimal("37"),
    ) is None


@pytest.mark.parametrize("ci,lcv,expected", [
    ("31", "19.9", "1.197"),
    ("20", "37", "2.633"),
    ("91.16", "37", "0.000"),
    ("95", "37", "0.000"),
    ("0", "37", "3.373"),
])
def test_equal_energy_lifecycle_comparison_does_not_create_cash_benefits(ci, lcv, expected):
    overlay = compute_listing_overlay(
        market_product=None,
        listing_ci_gco2_mj=Decimal(ci), listing_lcv_mj_kg=Decimal(lcv),
    )
    assert overlay is not None
    assert overlay.tco2e_avoided_per_mt == Decimal(expected)
    assert overlay.ci_basis == "LISTING"
    assert overlay.lcv_basis == "LISTING"
    assert overlay.penalty_avoided_eur_per_mt == Decimal("0.00")
    assert overlay.penalty_avoided_usd_per_mt == Decimal("0.00")


def test_physical_family_lcv_remains_explicitly_labelled():
    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL",
        listing_ci_gco2_mj=Decimal("31"), listing_lcv_mj_kg=None,
    )
    assert overlay is not None
    assert overlay.lcv_mj_kg == Decimal("19.9")
    assert overlay.lcv_basis == "PRODUCT_DEFAULT"
    assert overlay.ci_basis == "LISTING"


@pytest.mark.parametrize("ci,lcv", [
    ("31", None), ("31", "0"), ("31", "-1"), ("31", "NaN"), ("NaN", "37"),
])
def test_unresolvable_or_invalid_data_has_no_comparison(ci, lcv):
    assert compute_listing_overlay(
        market_product=None, listing_ci_gco2_mj=Decimal(ci),
        listing_lcv_mj_kg=Decimal(lcv) if lcv is not None else None,
    ) is None


@pytest.mark.parametrize("year,target", [
    (2025, "89.3368"), (2029, "89.3368"),
    (2030, "85.6904"), (2034, "85.6904"),
    (2035, "77.94180"), (2039, "77.94180"),
    (2040, "62.9004"), (2044, "62.9004"),
    (2045, "34.6408"), (2049, "34.6408"),
    (2050, "18.2320"), (2060, "18.2320"),
])
def test_year_target_steps_and_both_sides_of_each_boundary(year, target):
    assert fueleu_year_target(year) == Decimal(target)


def test_year_before_regulation_is_not_reported_as_2025():
    with pytest.raises(ValueError, match="starts in 2025"):
        fueleu_year_target(2024)


@pytest.mark.parametrize("vessel_count", [0, 3])
def test_vessels_do_not_turn_assumed_intensity_into_measured_fleet_data(vessel_count):
    assumptions = overlay_assumptions(year=2031, fleet_vessel_count=vessel_count)
    assert assumptions.fleet_intensity_basis == "DEFAULT_VLSFO"
    assert assumptions.fleet_vessel_count == vessel_count
    assert assumptions.ghgie_actual_gco2_mj == Decimal("91.16")
    assert assumptions.year_target == Decimal("85.6904")
    assert assumptions.excluded_factors == [
        "RFNBO_MULTIPLIER", "DEFICIT_ESCALATION", "EXTRA_EU_VOYAGE_SCOPE",
    ]


def test_eur_usd_override_does_not_create_unsubstantiated_cash_benefit(monkeypatch):
    monkeypatch.setattr(settings, "COMPLIANCE_EUR_USD_RATE", Decimal("1.10"))
    overlay = compute_listing_overlay(
        market_product="BIO_METHANOL", listing_ci_gco2_mj=Decimal("31"),
        listing_lcv_mj_kg=None,
    )
    assert overlay.penalty_avoided_usd_per_mt == Decimal("0.00")
    assert overlay_assumptions(2026, 0).eur_usd_rate == Decimal("1.10")
    assert EUR_USD_RATE == Decimal("1.08")


def test_json_preserves_decimal_string_contract_and_unknown_rows():
    priced_id, unknown_id = uuid4(), uuid4()
    response = PricingOverlayResponse(
        overlays={priced_id: compute_listing_overlay(
            market_product="BIO_METHANOL", listing_ci_gco2_mj=Decimal("31"),
            listing_lcv_mj_kg=None,
        ), unknown_id: None},
        assumptions=overlay_assumptions(year=2026, fleet_vessel_count=0),
    )
    payload = json.loads(response.model_dump_json())
    assert payload["overlays"][str(priced_id)]["penalty_avoided_usd_per_mt"] == "0.00"
    assert payload["overlays"][str(priced_id)]["tco2e_avoided_per_mt"] == "1.197"
    assert payload["overlays"][str(unknown_id)] is None


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
    provenance = (
        OrganizationProvenance.DEMO
        if org_id is not None and is_demo_market_organization(org_id)
        else OrganizationProvenance.REAL
    )
    org = Organization(
        name=f"{name}-{uuid4().hex[:6]}",
        type=org_type,
        provenance=provenance,
    )
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
    spec = PRODUCTS_BY_NAME.get(name)
    product = Product(
        id=spec.id if spec is not None else uuid4(),
        name=name,
        fuel_type=fuel_type,
        fuel_grade=fuel_grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    return product


async def _make_delivery_point(db: AsyncSession, name: str) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME.get(name)
    delivery_point = DeliveryPoint(
        id=spec.id if spec is not None else uuid4(),
        name=name,
        region=spec.region if spec is not None else "Asia",
        is_active=True,
    )
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
    provenance: OrganizationProvenance = OrganizationProvenance.REAL,
) -> OrderBookOrder:
    order = OrderBookOrder(
        organization_id=organization_id,
        provenance=provenance,
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
        expires_at=(
            datetime.now(UTC) + timedelta(days=1)
            if provenance == OrganizationProvenance.DEMO
            else None
        ),
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


async def test_endpoint_compares_visible_ask_without_financial_benefits(db: AsyncSession):
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
    assert row["financial_benefit_status"] == "UNPRICED"
    assert row["penalty_avoided_eur_per_mt"] == "0.00"
    assert row["penalty_avoided_usd_per_mt"] == "0.00"
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
    assert assumptions["year_target"] == "89.3368"
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
        provenance=OrganizationProvenance.DEMO,
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
    assert row["penalty_avoided_usd_per_mt"] == "0.00"


async def test_endpoint_registered_vessels_still_use_assumed_intensity(db: AsyncSession):
    buyer, supplier_org, product, singapore = await _marketplace_fixture(db)
    ask = await _make_order(
        db,
        organization_id=supplier_org.id,
        product_id=product.id,
        delivery_point_id=singapore.id,
    )
    # An ORM Vessel insert emits ST_GeogFromText() for the Geography columns,
    # which sqlite lacks; insert directly, binding uuids in the 32-char hex
    # format the dialect uses for UUID params on sqlite.
    await db.execute(
        text(
            "INSERT INTO vessels (id, organization_id, name, imo_number, updated_at) "
            "VALUES (:id, :org, 'MV Test', :imo, CURRENT_TIMESTAMP)"
        ),
        {
            "id": uuid4().hex,
            "org": buyer.organization_id.hex,
            "imo": f"IMO{uuid4().hex[:7]}",
        },
    )
    await db.commit()

    async with overlay_client(db, user=buyer) as client:
        response = await client.post(
            "/api/compliance/pricing-overlay",
            json={"order_ids": [str(ask.id)], "year": 2031},
        )

    assert response.status_code == 200
    payload = response.json()
    assumptions = payload["assumptions"]
    assert assumptions["fleet_intensity_basis"] == "DEFAULT_VLSFO"
    assert assumptions["fleet_vessel_count"] == 1
    # Registered vessels do not establish a measured fleet intensity.
    assert assumptions["ghgie_actual_gco2_mj"] == "91.16"
    assert assumptions["year"] == 2031
    assert assumptions["year_target"] == "85.6904"
    # year is annotation-only: the marginal math has no year term.
    assert payload["overlays"][str(ask.id)]["penalty_avoided_usd_per_mt"] == "0.00"


async def test_endpoint_rejects_pre_regulation_year(db: AsyncSession):
    buyer, _, _, _ = await _marketplace_fixture(db)
    await db.commit()
    async with overlay_client(db, user=buyer) as client:
        response = await client.post("/api/compliance/pricing-overlay", json={
            "order_ids": [], "year": 2024,
        })
    assert response.status_code == 422


async def test_endpoint_does_not_infer_ci_from_product(db: AsyncSession):
    buyer, supplier_org, product, singapore = await _marketplace_fixture(db)
    ask = await _make_order(db, organization_id=supplier_org.id,
        product_id=product.id, delivery_point_id=singapore.id, ci=None)
    await db.commit()
    async with overlay_client(db, user=buyer) as client:
        response = await client.post("/api/compliance/pricing-overlay", json={
            "order_ids": [str(ask.id)],
        })
    assert response.status_code == 200
    assert response.json()["overlays"][str(ask.id)] is None


def test_vessel_response_preserves_unpriced_ets_instead_of_zero_cash_cost():
    from app.routers.compliance_api import _score_to_response
    from app.services.compliance_scoring import calculate_compliance_score
    score = calculate_compliance_score("vessel", "No emissions data")
    payload = _score_to_response(score).model_dump(mode="json")
    assert payload["eu_ets"]["calculation_status"] == "UNPRICED"
    assert payload["eu_ets"]["total_co2_tonnes"] is None
    assert payload["eu_ets"]["estimated_cost_eur"] is None
