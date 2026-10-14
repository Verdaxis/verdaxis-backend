"""Direct LNG taking and public disclosure use the real route transaction."""
from decimal import Decimal
from unittest.mock import AsyncMock

import pytest
from fastapi import HTTPException
from sqlalchemy import select, func

from app.market_catalog import DELIVERY_POINT_IDS
from app.models.orderbook import Trade, TradeStatus
from app.models.user import OrgType, UserRole
from app.routers import trades, orderbook
from app.services.lng_order import lng_metadata_fields
from app.services.order_terms import order_terms_digest
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request, _make_ask, _make_delivery_point, _make_org, _make_product,
    _make_user, async_engine, setup_tables, db,
)
from tests.unit.test_lng_order import ASK, BID


@pytest.mark.asyncio
async def test_direct_take_conflict_and_confirmation_privacy(db, monkeypatch):
    monkeypatch.setattr(trades, "notify_org_users", AsyncMock())
    monkeypatch.setattr(trades, "track_analytics_event", lambda *args, **kwargs: None)
    from app.models.referral import Referral
    connection = await db.connection()
    await connection.run_sync(lambda sync: Referral.__table__.create(sync, checkfirst=True))
    buyer_org = await _make_org(db, "LNG buyer", OrgType.SHIPPING_LINE)
    seller_org = await _make_org(db, "LNG seller", OrgType.FUEL_TRADER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    seller = await _make_user(db, seller_org, UserRole.SUPPLIER)
    product = await _make_product(db, "Bio-LNG")
    point = await _make_delivery_point(db)
    point.id = DELIVERY_POINT_IDS["Rotterdam"]
    point.name, point.region = "Rotterdam", "Europe"
    await db.flush()
    ask = await _make_ask(db, org_id=seller_org.id, product_id=product.id,
                          delivery_point_id=point.id, owner_user_id=seller.id)
    ask.lng_terms = dict(ASK)
    for field, value in lng_metadata_fields(ASK).items():
        setattr(ask, field, value)
    await db.commit()
    public = await orderbook._order_response(db, ask, benchmark_cache={})
    assert "scheme_certificate_number" not in public.lng_terms.model_dump()
    assert "physical_supplier_name" not in public.lng_terms.model_dump()
    payload = trades.TradeCreate(order_id=ask.id, quantity_mt=1,
        expected_terms_digest=order_terms_digest(ask), lng_terms={**BID, "accept_resellers": False})
    with pytest.raises(HTTPException) as rejected:
        await trades.create_trade(payload=payload, request=_fake_request(), db=db, current_user=buyer)
    assert rejected.value.status_code == 409
    assert rejected.value.detail == {"code": "LNG_TERMS_INCOMPATIBLE"}
    assert ask.remaining_quantity_mt == Decimal("1000")
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0
    response = await trades.create_trade(
        payload=trades.TradeCreate(order_id=ask.id, quantity_mt=1,
            expected_terms_digest=order_terms_digest(ask), lng_terms=BID),
        request=_fake_request(), db=db, current_user=buyer,
    )
    assert "physical_supplier_name" not in response.lng_terms_snapshot.ask.model_dump()
    confirmed = await trades.confirm_trade(trade_id=response.id, request=_fake_request(), db=db, current_user=seller)
    assert confirmed.status == TradeStatus.CONFIRMED
    assert confirmed.lng_terms_snapshot.ask.physical_supplier_name == ASK["physical_supplier_name"]
