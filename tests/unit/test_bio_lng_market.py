"""Bio-LNG market slice (docs/plans/2026-10-14-bio-lng-design.md)."""
from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError
from sqlalchemy import func, select

from app.market_catalog import (
    BIO_LNG_PRODUCT_ID,
    DELIVERY_POINT_IDS,
    DELIVERY_POINTS_BY_NAME,
    PRODUCT_IDS,
    product_delivery_point_error,
)
from app.models.catalog import DeliveryPoint
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.user import OrganizationProvenance, OrgType, UserRole
from app.reference.iso_countries import ISO_3166_1_ALPHA_2
from app.routers import trades
from app.routers.orderbook import create_order
from app.schemas.lng_terms import LngAskTerms, LngBidTerms
from app.schemas.orderbook import OrderCreate, OrderMyResponse, OrderResponse
from app.services import matching_engine
from app.services.ci_pricing import ci_adjusted_price_for_order
from app.services.execution_policy import order_is_execution_qualified
from app.services.lng_terms import (
    LNG_TERMS_INCOMPATIBLE,
    derived_order_fields,
    lng_terms_compatible,
    validate_order_lng_terms,
)
from app.services.market_data_eligibility import (
    market_product_supports_delivery_point,
    public_order_is_visible,
)
from app.services.order_terms import order_terms_digest
from tests.unit.fixtures import bio_lng
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request,
    _make_org,
    _make_product,
    _make_user,
    async_engine,  # noqa: F401
    db,  # noqa: F401
    setup_tables,  # noqa: F401
)

ROTTERDAM_ID = DELIVERY_POINT_IDS["Rotterdam"]
SINGAPORE_ID = DELIVERY_POINT_IDS["Singapore"]
IDENTITY_FIELDS = ("scheme_certificate_number", "physical_supplier_name")


# ---------------------------------------------------------------------------
# Terms validation and scalar derivation
# ---------------------------------------------------------------------------

def test_valid_terms_are_normalized_for_the_order_side():
    ask = validate_order_lng_terms(BIO_LNG_PRODUCT_ID, OrderSide.ASK, bio_lng.ask_terms())
    bid = validate_order_lng_terms(
        BIO_LNG_PRODUCT_ID,
        "BID",
        {"side": "BID", "accepted_schemes": ["REDCERT_EU"], "accepted_delivery_methods": ["SHIP_TO_SHIP"]},
    )

    assert ask["wtt_ghg_gco2e_mj"] == "-45.50"
    assert bid["accepted_custody"] == ["MASS_BALANCE", "PHYSICAL_SEGREGATION"]
    assert bid["require_union_database"] is True
    assert bid["accept_resellers"] is True


@pytest.mark.parametrize(
    "overrides",
    [
        {"bio_share_pct": "0.99"},
        {"bio_share_pct": "50.125"},
        {"wtt_ghg_gco2e_mj": "-150.01"},
        {"wtt_ghg_gco2e_mj": "100.01"},
        {"production_country": "nl"},
        {"production_country": "NLD"},
        {"specification_standard": "ISO 8217:2024"},
        {"scheme_certificate_number": "AB"},
        {"physical_supplier_name": "X"},
        {"feedstock": "PALM_OIL"},
        {"min_methane_number": "101"},
        {"union_database_registered": "yes"},
        {"unexpected": True},
    ],
)
def test_invalid_ask_declarations_are_rejected(overrides):
    with pytest.raises(ValidationError):
        LngAskTerms.model_validate(bio_lng.ask_terms(**overrides))


# The origin the server derives comes straight from production_country, so the
# schema must accept only codes that the ISO 3166-1 standard assigns to a
# country. Well-formed but unassigned codes must not become an order origin.
@pytest.mark.parametrize("code", ["DE", "NL", "US", "SG"])
def test_assigned_production_countries_are_accepted(code):
    terms = LngAskTerms.model_validate(bio_lng.ask_terms(production_country=code))

    assert terms.production_country == code


@pytest.mark.parametrize("code", ["ZZ", "AA", "XK", "UK", "EU", "de"])
def test_unassigned_or_reserved_production_countries_are_rejected(code):
    # Lower-case input fails the field pattern; the other codes are well formed
    # but name no country, so the reference-set check must reject them.
    with pytest.raises(ValidationError):
        LngAskTerms.model_validate(bio_lng.ask_terms(production_country=code))


def test_unassigned_but_well_formed_production_country_names_its_rule():
    with pytest.raises(ValidationError, match="officially assigned ISO 3166-1"):
        LngAskTerms.model_validate(bio_lng.ask_terms(production_country="ZZ"))


def test_iso_country_reference_set_holds_only_the_249_assigned_codes():
    assert len(ISO_3166_1_ALPHA_2) == 249
    assert {"DE", "NL", "US", "SG"} <= ISO_3166_1_ALPHA_2
    assert not {"ZZ", "AA", "XK", "UK", "EU"} & ISO_3166_1_ALPHA_2


@pytest.mark.parametrize(
    "overrides",
    [
        {"accepted_schemes": []},
        {"accepted_schemes": ["ISCC_EU", "ISCC_EU"]},
        {"accepted_custody": []},
        {"accepted_delivery_methods": []},
        {"min_bio_share_pct": "0"},
        {"max_wtt_ghg_gco2e_mj": "-151"},
    ],
)
def test_invalid_bid_requirements_are_rejected(overrides):
    with pytest.raises(ValidationError):
        LngBidTerms.model_validate(bio_lng.bid_terms(**overrides))


@pytest.mark.parametrize(
    ("product_id", "side", "terms", "message"),
    [
        (BIO_LNG_PRODUCT_ID, "ASK", None, "require lng_terms"),
        (BIO_LNG_PRODUCT_ID, "BID", bio_lng.ask_terms(), "must match the order side"),
        (BIO_LNG_PRODUCT_ID, "ASK", {"side": "ASK"}, "not valid Bio-LNG terms"),
        (PRODUCT_IDS["B100"], "ASK", bio_lng.ask_terms(), "only valid for Bio-LNG"),
    ],
)
def test_terms_are_required_on_bio_lng_and_rejected_elsewhere(product_id, side, terms, message):
    with pytest.raises(HTTPException, match=message) as failure:
        validate_order_lng_terms(product_id, side, terms)
    assert failure.value.status_code == 422


def test_scalars_are_derived_from_terms_and_scalar_ci_stays_null():
    ask_fields = derived_order_fields("ASK", bio_lng.ask_terms(sustainability_scheme="REDCERT_EU"))
    bid_fields = derived_order_fields("BID", bio_lng.bid_terms())

    assert ask_fields == {
        "certification_declared": True,
        "certification_scheme": "REDcert-EU",
        "certifications": ["REDcert-EU"],
        "specification_standard": "ISO 23306:2020",
        "feedstock": "Manure",
        "origin": "NL",
        "carbon_intensity_gco2_mj": None,
        "carbon_intensity_method": None,
    }
    assert bid_fields == {"certification_scheme": None, "certifications": ["ISCC EU", "REDcert-EU"]}


# ---------------------------------------------------------------------------
# Compatibility (design section 3, rules 1-8)
# ---------------------------------------------------------------------------

@pytest.mark.parametrize(
    ("bid_overrides", "ask_overrides", "compatible"),
    [
        ({}, {}, True),
        ({"accepted_schemes": ["REDCERT_EU"]}, {}, False),  # rule 1
        ({"accepted_custody": ["PHYSICAL_SEGREGATION"]}, {}, False),  # rule 2
        ({}, {"union_database_registered": False}, False),  # rule 3
        ({"require_union_database": False}, {"union_database_registered": False}, True),
        ({"min_bio_share_pct": "100"}, {"bio_share_pct": "99.99"}, False),  # rule 4
        ({"min_bio_share_pct": None}, {"bio_share_pct": "1.00"}, True),
        ({"max_wtt_ghg_gco2e_mj": "-50"}, {}, False),  # rule 5
        ({"max_wtt_ghg_gco2e_mj": "-45.5"}, {}, True),
        ({"accepted_delivery_methods": ["SHIP_TO_SHIP"]}, {}, False),  # rule 6
        ({"min_methane_number": "81"}, {}, False),  # rule 7
        ({}, {"min_methane_number": None}, False),
        ({"min_methane_number": None}, {"min_methane_number": None}, True),
        ({"accept_resellers": False}, {}, False),  # rule 8
        ({"accept_resellers": False}, {"seller_is_physical_supplier": True}, True),
    ],
)
def test_compatibility_table(bid_overrides, ask_overrides, compatible):
    bid = bio_lng.bid_terms(**bid_overrides)
    ask = bio_lng.ask_terms(**ask_overrides)

    assert lng_terms_compatible(bid, ask) is compatible
    assert lng_terms_compatible(LngBidTerms.model_validate(bid), LngAskTerms.model_validate(ask)) is compatible


def test_missing_or_invalid_terms_are_never_compatible():
    assert not lng_terms_compatible(None, bio_lng.ask_terms())
    assert not lng_terms_compatible(bio_lng.bid_terms(), {"side": "ASK"})


# ---------------------------------------------------------------------------
# Lane, public view, and CI pricing
# ---------------------------------------------------------------------------

def _supplier():
    user = MagicMock()
    user.id = uuid4()
    user.role = UserRole.SUPPLIER
    user.organization_id = uuid4()
    return user


@pytest.mark.asyncio
async def test_order_entry_rejects_bio_lng_outside_rotterdam():
    order = OrderCreate(
        side=OrderSide.ASK,
        product_id=BIO_LNG_PRODUCT_ID,
        delivery_point_id=SINGAPORE_ID,
        quantity_mt=Decimal("500"),
        price_per_mt_usd=Decimal("1500"),
        msds_available=True,
        lng_terms=bio_lng.ask_terms(),
    )

    db = AsyncMock()
    db.execute.return_value = MagicMock()  # the product and delivery point both exist

    with pytest.raises(HTTPException, match="Bio-LNG is available at Rotterdam only") as failure:
        await create_order(
            request=_fake_request(), order_data=order, current_user=_supplier(), db=db,
        )

    assert failure.value.status_code == 400
    assert product_delivery_point_error(BIO_LNG_PRODUCT_ID, ROTTERDAM_ID) is None
    assert product_delivery_point_error(PRODUCT_IDS["B30"], SINGAPORE_ID) is None
    assert market_product_supports_delivery_point("BIO_LNG", ROTTERDAM_ID)
    assert not market_product_supports_delivery_point("BIO_LNG", SINGAPORE_ID)


def _bio_lng_ask(*, organization_id, delivery_point_id, owner_user_id=None, **overrides):
    terms = bio_lng.ask_terms(**overrides)
    return OrderBookOrder(
        organization_id=organization_id,
        owner_user_id=owner_user_id,
        provenance=OrganizationProvenance.REAL,
        side=OrderSide.ASK,
        product_id=BIO_LNG_PRODUCT_ID,
        delivery_point_id=delivery_point_id,
        quantity_mt=Decimal("1000"),
        remaining_quantity_mt=Decimal("1000"),
        price_per_mt_usd=Decimal("1500"),
        availability_window="SPOT",
        status=OrderBookStatus.OPEN,
        created_at=datetime.now(UTC),
        msds_available=True,
        off_spec=False,
        lng_terms=terms,
        **derived_order_fields("ASK", terms),
    )


async def _make_point(db, name: str) -> DeliveryPoint:
    spec = DELIVERY_POINTS_BY_NAME[name]
    point = DeliveryPoint(id=spec.id, name=spec.name, region=spec.region, is_active=True)
    db.add(point)
    await db.flush()
    return point


@pytest.mark.asyncio
async def test_public_reads_show_bio_lng_only_at_rotterdam(db):
    seller_org = await _make_org(db, "Bio-LNG seller", OrgType.FUEL_TRADER)
    await _make_product(db, "Bio-LNG")
    for name in ("Rotterdam", "Singapore"):
        await _make_point(db, name)
    rotterdam_ask = _bio_lng_ask(organization_id=seller_org.id, delivery_point_id=ROTTERDAM_ID)
    # Written directly: the API and the database constraint refuse this lane.
    singapore_ask = _bio_lng_ask(organization_id=seller_org.id, delivery_point_id=SINGAPORE_ID)
    db.add_all([rotterdam_ask, singapore_ask])
    await db.flush()

    assert await public_order_is_visible(db, rotterdam_ask.id)
    assert not await public_order_is_visible(db, singapore_ask.id)
    assert order_is_execution_qualified(rotterdam_ask)
    assert not order_is_execution_qualified(singapore_ask)


def test_public_order_view_hides_seller_identity_but_owner_view_keeps_it():
    order = _bio_lng_ask(
        organization_id=uuid4(), delivery_point_id=ROTTERDAM_ID, union_database_registered=False,
    )
    order.id = uuid4()
    order.is_verdaxis_verified = False
    order.updated_at = order.created_at
    order.creation_method = OrderCreationMethod.SELF_SERVICE
    order.version = 1

    public = OrderResponse.model_validate(order, from_attributes=True).model_dump(mode="json")
    owner = OrderMyResponse.model_validate(order, from_attributes=True).model_dump(mode="json")

    assert not set(IDENTITY_FIELDS) & set(public["lng_terms"])
    assert public["lng_terms"]["reseller"] is True
    assert public["lng_terms"]["fueleu_recognition"] == "NOT_REGISTERED"
    assert public["carbon_intensity_gco2_mj"] is None
    assert owner["lng_terms"]["scheme_certificate_number"] == "EU-ISCC-Cert-NL201-12345"
    assert owner["lng_terms"]["physical_supplier_name"] == "Rotterdam Bunker Partner B.V."


def test_ci_pricing_skips_bio_lng_even_with_scalar_values():
    order = SimpleNamespace(
        product_id=BIO_LNG_PRODUCT_ID,
        price_per_mt_usd=Decimal("1500"),
        carbon_intensity_gco2_mj=Decimal("10"),
        energy_density_mj_kg=Decimal("50"),
    )
    assert ci_adjusted_price_for_order(order) is None

    order.product_id = PRODUCT_IDS["BIO_METHANOL"]
    assert ci_adjusted_price_for_order(order) is not None


# ---------------------------------------------------------------------------
# Matching and direct takes share one compatibility function
# ---------------------------------------------------------------------------

async def _bio_lng_market(db):
    buyer_org = await _make_org(db, "Bio-LNG buyer", OrgType.SHIPPING_LINE)
    seller_org = await _make_org(db, "Bio-LNG trader", OrgType.FUEL_TRADER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    seller = await _make_user(db, seller_org, UserRole.SUPPLIER)
    await _make_product(db, "Bio-LNG")
    await _make_point(db, "Rotterdam")
    ask = _bio_lng_ask(
        organization_id=seller_org.id, delivery_point_id=ROTTERDAM_ID, owner_user_id=seller.id,
    )
    db.add(ask)
    await db.flush()
    await db.refresh(ask, ["product", "delivery_point"])
    return buyer, seller, ask


def _bio_lng_bid(*, buyer, terms) -> OrderBookOrder:
    return OrderBookOrder(
        organization_id=buyer.organization_id,
        owner_user_id=buyer.id,
        provenance=OrganizationProvenance.REAL,
        side=OrderSide.BID,
        product_id=BIO_LNG_PRODUCT_ID,
        delivery_point_id=ROTTERDAM_ID,
        quantity_mt=Decimal("400"),
        remaining_quantity_mt=Decimal("400"),
        price_per_mt_usd=Decimal("1600"),
        availability_window="SPOT",
        status=OrderBookStatus.OPEN,
        lng_terms=terms,
        **derived_order_fields("BID", terms),
    )


@pytest.mark.asyncio
@pytest.mark.parametrize("compatible", [False, True])
async def test_matching_requires_compatible_terms_and_freezes_both_sides(monkeypatch, db, compatible):
    monkeypatch.setattr(matching_engine, "notify_org_users_batched", AsyncMock())
    buyer, _seller, ask = await _bio_lng_market(db)
    terms = bio_lng.bid_terms(accept_resellers=compatible)
    bid = _bio_lng_bid(buyer=buyer, terms=terms)
    db.add(bid)
    await db.flush()
    await db.refresh(bid, ["product", "delivery_point"])

    matched = await matching_engine.match_order(db, bid)

    if not compatible:
        assert matched == []
        assert ask.remaining_quantity_mt == Decimal("1000")
        return
    [trade] = matched
    assert trade.status == TradeStatus.CONFIRMED
    assert trade.lng_terms_snapshot == {
        "bid": LngBidTerms.model_validate(terms).model_dump(mode="json"),
        "ask": ask.lng_terms,
    }


@pytest.mark.asyncio
async def test_direct_take_with_incompatible_requirements_returns_stable_code(monkeypatch, db):
    monkeypatch.setattr(trades, "notify_org_users", AsyncMock())
    buyer, _seller, ask = await _bio_lng_market(db)

    with pytest.raises(HTTPException) as failure:
        await trades.create_trade(
            payload=trades.TradeCreate(
                order_id=ask.id,
                quantity_mt=Decimal("100"),
                expected_terms_digest=order_terms_digest(ask),
                lng_terms=bio_lng.bid_terms(accepted_schemes=["REDCERT_EU"]),
            ),
            request=_fake_request(), db=db, current_user=buyer,
        )

    assert failure.value.status_code == 409
    assert failure.value.detail["code"] == LNG_TERMS_INCOMPATIBLE
    assert "Rotterdam Bunker Partner" not in str(failure.value.detail)
    assert ask.remaining_quantity_mt == Decimal("1000")
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0


@pytest.mark.asyncio
async def test_direct_take_freezes_terms_and_reveals_full_ask_only_after_confirmation(monkeypatch, db):
    monkeypatch.setattr(trades, "notify_org_users", AsyncMock())
    buyer, seller, ask = await _bio_lng_market(db)

    pending = await trades.create_trade(
        payload=trades.TradeCreate(
            order_id=ask.id,
            quantity_mt=Decimal("100"),
            expected_terms_digest=order_terms_digest(ask),
            lng_terms=bio_lng.bid_terms(),
        ),
        request=_fake_request(), db=db, current_user=buyer,
    )
    trade = await db.get(Trade, pending.id)

    assert pending.status == "PENDING_CONFIRMATION"
    assert not set(IDENTITY_FIELDS) & set(pending.lng_terms_snapshot.ask.model_dump())
    seller_view = trades.build_trade_response(trade, viewer_org_id=seller.organization_id)
    assert seller_view.lng_terms_snapshot.ask.physical_supplier_name == "Rotterdam Bunker Partner B.V."

    trade.status = TradeStatus.CONFIRMED
    buyer_view = trades.build_trade_response(trade, viewer_org_id=buyer.organization_id)
    outsider_view = trades.build_trade_response(trade, viewer_org_id=uuid4())
    assert buyer_view.lng_terms_snapshot.ask.scheme_certificate_number == "EU-ISCC-Cert-NL201-12345"
    assert not set(IDENTITY_FIELDS) & set(outsider_view.lng_terms_snapshot.ask.model_dump())


@pytest.mark.asyncio
async def test_direct_take_of_bio_lng_requires_buyer_terms_and_bids_require_a_resting_ask(monkeypatch, db):
    monkeypatch.setattr(trades, "notify_org_users", AsyncMock())
    buyer, seller, ask = await _bio_lng_market(db)
    bid = _bio_lng_bid(buyer=buyer, terms=bio_lng.bid_terms())
    db.add(bid)
    await db.flush()
    await db.refresh(bid, ["product", "delivery_point"])

    with pytest.raises(HTTPException, match="require lng_terms") as missing_terms:
        await trades.create_trade(
            payload=trades.TradeCreate(
                order_id=ask.id, quantity_mt=Decimal("100"),
                expected_terms_digest=order_terms_digest(ask),
            ),
            request=_fake_request(), db=db, current_user=buyer,
        )
    with pytest.raises(HTTPException, match="Bio-LNG ASK order") as bid_fill:
        await trades.create_trade(
            payload=trades.TradeCreate(
                order_id=bid.id, quantity_mt=Decimal("100"),
                expected_terms_digest=order_terms_digest(bid),
            ),
            request=_fake_request(), db=db, current_user=seller,
        )

    assert missing_terms.value.status_code == 422
    assert bid_fill.value.status_code == 400
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0
