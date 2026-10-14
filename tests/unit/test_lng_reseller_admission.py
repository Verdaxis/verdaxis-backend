"""Interim reseller admission leaves typed terms and matching unchanged."""


from datetime import UTC, datetime
from unittest.mock import AsyncMock
from uuid import uuid4

import pytest
from fastapi import HTTPException
from sqlalchemy import func, select

from app.market_catalog import DELIVERY_POINT_IDS
from app.models.orderbook import OrderBookOrder
from app.models.user import OrgType, UserRole
from app.routers import orderbook
from app.schemas.orderbook import OrderCreate, OrderUpdate
from app.services.request_party import RequestParty, RequestPartyMode
from tests.unit.test_lng_order import ASK
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request,
    _make_delivery_point,
    _make_org,
    _make_product,
    _make_user,
)
from tests.unit.test_trade_watchlist_hooks import async_engine as async_engine
from tests.unit.test_trade_watchlist_hooks import db as db
from tests.unit.test_trade_watchlist_hooks import setup_tables as setup_tables

RESELLER_DETAIL = {
    "code": "RESELLER_ALLOCATION_REQUIRED",
    "message": "Reseller listings need a verified supplier allocation. Contact Verdaxis to register one.",
}


def test_market_support_candidate_rejects_reseller_authorization():
    from types import SimpleNamespace

    from app.market_catalog import PRODUCT_IDS
    from app.routers import market_support

    authorization = SimpleNamespace(product_id=PRODUCT_IDS["BIO_LNG"], lng_terms=ASK)
    with pytest.raises(HTTPException) as error:
        market_support._candidate_from_authorization(authorization, None, None, None)
    assert error.value.status_code == 422
    assert error.value.detail == RESELLER_DETAIL


@pytest.fixture
async def lng_market(db, monkeypatch):
    monkeypatch.setattr(orderbook, "track_analytics_event", lambda *args, **kwargs: None)
    org = await _make_org(db, "LNG seller", OrgType.FUEL_TRADER)
    seller = await _make_user(db, org, UserRole.SUPPLIER)
    product = await _make_product(db, "Bio-LNG")
    point = await _make_delivery_point(db)
    point.id = DELIVERY_POINT_IDS["Rotterdam"]
    point.name, point.region = "Rotterdam", "Europe"
    await db.commit()
    return seller, {
        "side": "ASK", "product_id": product.id, "delivery_point_id": point.id,
        "quantity_mt": 100, "price_per_mt_usd": 1000, "availability_window": "SPOT",
        "msds_available": True, "lng_terms": dict(ASK),
    }


@pytest.mark.asyncio
@pytest.mark.parametrize("assisted", [False, True])
async def test_reseller_ask_create_rejected_before_insert(db, lng_market, monkeypatch, assisted):
    seller, payload = lng_market
    request = _fake_request()
    if assisted:
        party = RequestParty(
            actor=seller, effective_organization=None, accountable_principal=seller,
            effective_role=UserRole.SUPPLIER, mode=RequestPartyMode.MARKET_SUPPORT,
            support_context_id=uuid4(),
        )
        monkeypatch.setattr(orderbook, "resolve_request_party", AsyncMock(return_value=party))
        request.headers["Idempotency-Key"] = str(uuid4())
        payload["support_confirmation"] = {
            "external_instruction_reference": "Customer instruction 14 October",
            "instruction_at": datetime.now(UTC),
            "acknowledge_exact_terms": True,
            "acknowledge_executable_standing_order": True,
        }
    with pytest.raises(HTTPException) as error:
        await orderbook.create_order(request=request, order_data=OrderCreate(**payload),
                                     current_user=seller, db=db)
    assert error.value.status_code == 422
    assert error.value.detail == RESELLER_DETAIL
    assert await db.scalar(select(func.count()).select_from(OrderBookOrder)) == 0


@pytest.mark.asyncio
async def test_physical_supplier_posts_but_reseller_amend_changes_nothing(db, lng_market):
    seller, payload = lng_market
    payload["lng_terms"]["seller_is_physical_supplier"] = True
    created = await orderbook.create_order(
        request=_fake_request(), order_data=OrderCreate(**payload), current_user=seller, db=db,
    )
    stored = await db.get(OrderBookOrder, created.id)
    original_terms = dict(stored.lng_terms)
    with pytest.raises(HTTPException) as error:
        await orderbook.update_order(
            order_id=created.id, request=_fake_request(),
            update_data=OrderUpdate(lng_terms=ASK, price_per_mt_usd=900),
            current_user=seller, db=db,
        )
    assert error.value.status_code == 422
    assert error.value.detail == RESELLER_DETAIL
    await db.refresh(stored)
    assert stored.lng_terms == original_terms
    assert stored.price_per_mt_usd == 1000
