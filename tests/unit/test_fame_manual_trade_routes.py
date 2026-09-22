"""Exercise manual B100 trade routes with a real isolated SQLAlchemy session."""

from copy import deepcopy
from datetime import datetime, timedelta
from decimal import Decimal
from unittest.mock import AsyncMock
from zoneinfo import ZoneInfo

import pytest
from fastapi import HTTPException
from sqlalchemy import func, select

from app.market_catalog import PRODUCTS_BY_NAME
from app.models.catalog import Product
from app.models.orderbook import OrderSide, Trade, TradeStatus
from app.models.user import OrgType, UserRole
from app.routers import trades
from app.schemas.fame_order import FameAskTerms, FameBidTerms
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request,
    _make_ask,
    _make_delivery_point,
    _make_org,
    _make_user,
    async_engine,  # noqa: F401 -- shared disposable SQLite fixtures
    setup_tables,  # noqa: F401
)
from tests.unit.test_trade_watchlist_hooks import (
    db as db,  # noqa: PLC0414 -- expose the shared pytest fixture
)


@pytest.fixture(autouse=True)
def isolate_notifications(monkeypatch):
    monkeypatch.setattr(trades, "notify_org_users", AsyncMock())
    monkeypatch.setattr(trades, "track_analytics_event", lambda *args, **kwargs: None)


@pytest.fixture
def fuel_pair():
    bid = FameBidTerms(
        side="BID",
        neat_fame=True,
        standard="EN_14214",
        standard_edition="2012+A2:2019",
        max_cfpp_c=-5,
        sustainability_scheme="ISCC_EU",
    )
    ask = FameAskTerms(
        side="ASK",
        neat_fame=True,
        uco_mass_pct=100,
        evidence_due="BEFORE_LOADING",
        standard="EN_14214",
        standard_edition="2012+A2:2019",
        cfpp_c=-10,
        sustainability_scheme="ISCC_EU",
        certificate_reference="private-certificate-123",
        certificate_holder="Supplier Legal Name",
        certificate_valid_until=datetime.now(ZoneInfo("Asia/Singapore")).date()
        + timedelta(days=90),
        evidence_status="PENDING",
    )
    return bid, ask


async def _market(db, fuel_pair, *, side=OrderSide.ASK):
    buyer_org = await _make_org(db, "B100 buyer", OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, "B100 supplier", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    product_spec = PRODUCTS_BY_NAME["UCOME B100"]
    product = Product(
        id=product_spec.id,
        name=product_spec.name,
        fuel_type=product_spec.fuel_type,
        fuel_grade=product_spec.fuel_grade,
        is_active=True,
    )
    db.add(product)
    await db.flush()
    point = await _make_delivery_point(db)
    order = await _make_ask(
        db,
        org_id=supplier_org.id if side == OrderSide.ASK else buyer_org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        owner_user_id=supplier.id if side == OrderSide.ASK else buyer.id,
        certification_scheme="ISCC_EU",
    )
    order.side = side
    order.fame_terms = fuel_pair[1 if side == OrderSide.ASK else 0].model_dump(
        mode="json"
    )
    order.specification_standard = "EN_14214 2012+A2:2019"
    await db.commit()
    return order, buyer, supplier


@pytest.mark.asyncio
@pytest.mark.parametrize("request_kind", ["missing", "incompatible"])
async def test_rejected_b100_take_does_not_consume_capacity(
    db, fuel_pair, request_kind
):
    order, buyer, _ = await _market(db, fuel_pair)
    bid, _ = fuel_pair
    terms = None if request_kind == "missing" else bid
    if request_kind == "incompatible":
        terms = bid.model_copy(update={"max_cfpp_c": Decimal(-20)})
    payload = trades.TradeCreate(
        order_id=order.id,
        quantity_mt="100",
        fame_terms=terms,
        expected_order_version=order.version,
    )
    with pytest.raises(HTTPException) as error:
        await trades.create_trade(
            payload=payload, request=_fake_request(), db=db, current_user=buyer
        )
    assert error.value.status_code == 422
    assert order.remaining_quantity_mt == Decimal(1000)
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0


@pytest.mark.asyncio
async def test_b100_partial_take_below_lot_minimum_keeps_terms_and_supplier_private(
    db, fuel_pair
):
    order, buyer, supplier = await _market(db, fuel_pair)
    response = await trades.create_trade(
        payload=trades.TradeCreate(
            order_id=order.id,
            quantity_mt="0.50",
            fame_terms=fuel_pair[0],
            expected_order_version=order.version,
        ),
        request=_fake_request(),
        db=db,
        current_user=buyer,
    )
    trade = await db.get(Trade, response.id)
    assert trade.status == TradeStatus.PENDING_CONFIRMATION
    assert order.remaining_quantity_mt == Decimal("999.50")
    assert trade.fame_terms_snapshot["ask"] == fuel_pair[1].model_dump(mode="json")
    assert (
        response.fame_terms_snapshot.ask.model_dump().get("certificate_holder") is None
    )
    seller_view = trades.build_trade_response(
        trade, viewer_org_id=supplier.organization_id
    )
    assert (
        seller_view.fame_terms_snapshot.ask.certificate_holder == "Supplier Legal Name"
    )


@pytest.mark.asyncio
async def test_supplier_taking_b100_bid_requires_explicit_declarations(db, fuel_pair):
    order, _, supplier = await _market(db, fuel_pair, side=OrderSide.BID)
    payload = trades.TradeCreate(
        order_id=order.id,
        quantity_mt=100,
        fame_terms=fuel_pair[1],
        expected_order_version=order.version,
    )
    with pytest.raises(HTTPException) as error:
        await trades.create_trade(
            payload=payload, request=_fake_request(), db=db, current_user=supplier
        )
    assert error.value.status_code == 422
    assert order.remaining_quantity_mt == Decimal(1000)
    response = await trades.create_trade(
        payload=payload.model_copy(
            update={"certification_declared": True, "msds_available": True}
        ),
        request=_fake_request(),
        db=db,
        current_user=supplier,
    )
    assert response.fame_terms_snapshot.ask.certificate_holder == "Supplier Legal Name"
    assert order.remaining_quantity_mt == Decimal(900)


@pytest.mark.asyncio
async def test_confirmation_rejects_changed_b100_terms_without_confirming(
    db, fuel_pair
):
    order, buyer, supplier = await _market(db, fuel_pair)
    response = await trades.create_trade(
        payload=trades.TradeCreate(
            order_id=order.id,
            quantity_mt=100,
            fame_terms=fuel_pair[0],
            expected_order_version=order.version,
        ),
        request=_fake_request(),
        db=db,
        current_user=buyer,
    )
    changed_terms = deepcopy(order.fame_terms)
    changed_terms["cfpp_c"] = "-6"
    order.fame_terms = changed_terms
    await db.commit()
    with pytest.raises(HTTPException) as error:
        await trades.confirm_trade(
            trade_id=response.id, request=_fake_request(), db=db, current_user=supplier
        )
    assert error.value.status_code == 409
    trade = await db.get(Trade, response.id)
    assert trade.status == TradeStatus.PENDING_CONFIRMATION
    assert trade.confirmed_at is None
    assert trade.fame_terms_snapshot["ask"]["cfpp_c"] == "-10"


@pytest.mark.asyncio
@pytest.mark.parametrize("side", [OrderSide.ASK, OrderSide.BID])
@pytest.mark.parametrize("reviewed_version", [None, 1])
async def test_b100_take_requires_review_of_current_source_version(
    db, fuel_pair, side, reviewed_version
):
    order, buyer, supplier = await _market(db, fuel_pair, side=side)
    original_version = order.version
    order.bump_version()
    await db.commit()
    payload = trades.TradeCreate(
        order_id=order.id,
        quantity_mt=100,
        fame_terms=fuel_pair[0 if side == OrderSide.ASK else 1],
        certification_declared=side == OrderSide.BID,
        msds_available=side == OrderSide.BID,
        expected_order_version=original_version
        if reviewed_version is not None
        else None,
    )
    actor = buyer if side == OrderSide.ASK else supplier
    with pytest.raises(HTTPException) as error:
        await trades.create_trade(
            payload=payload, request=_fake_request(), db=db, current_user=actor
        )
    assert error.value.status_code == (409 if reviewed_version is not None else 422)
    assert order.remaining_quantity_mt == Decimal(1000)
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0

    response = await trades.create_trade(
        payload=payload.model_copy(update={"expected_order_version": order.version}),
        request=_fake_request(),
        db=db,
        current_user=actor,
    )
    assert response.fame_terms_snapshot is not None
    assert order.remaining_quantity_mt == Decimal(900)
