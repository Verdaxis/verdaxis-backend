"""Direct hits must bind the exact reviewed order terms."""

from decimal import Decimal

import pytest
from fastapi import HTTPException
from sqlalchemy import func, select

from app.models.orderbook import Trade
from app.models.user import OrgType, UserRole
from app.routers import trades
from app.services.idempotency import TRADE_CREATE_OPERATION, idempotency_request_hash
from app.services.order_terms import order_terms_digest
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request,
    _make_ask,
    _make_delivery_point,
    _make_org,
    _make_product,
    _make_user,
    _pending_trade,
    async_engine,  # noqa: F401
    db,  # noqa: F401
    setup_tables,  # noqa: F401
)


@pytest.mark.asyncio
@pytest.mark.parametrize("reviewed_digest", [None, "stale"])
async def test_new_direct_hit_requires_current_reviewed_terms(db, reviewed_digest):
    buyer_org = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    seller_org = await _make_org(db, "Seller", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    seller = await _make_user(db, seller_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    order = await _make_ask(
        db,
        org_id=seller_org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        owner_user_id=seller.id,
    )
    original_digest = order_terms_digest(order)
    order.price_per_mt_usd += Decimal("1")
    await db.flush()

    with pytest.raises(HTTPException) as failure:
        await trades.create_trade(
            payload=trades.TradeCreate(
                order_id=order.id,
                quantity_mt="100",
                expected_terms_digest=(
                    original_digest if reviewed_digest == "stale" else None
                ),
            ),
            request=_fake_request(),
            db=db,
            current_user=buyer,
        )

    assert failure.value.status_code == 409
    assert failure.value.detail["code"] == trades.ORDER_TERMS_REVIEW_REQUIRED
    assert order.remaining_quantity_mt == Decimal("1000")
    assert await db.scalar(select(func.count()).select_from(Trade)) == 0


@pytest.mark.asyncio
async def test_successful_old_replay_precedes_current_terms_check(db):
    buyer_org = await _make_org(db, "Replay buyer", OrgType.SHIPPING_LINE)
    seller_org = await _make_org(db, "Replay seller", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    seller = await _make_user(db, seller_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    order = await _make_ask(
        db,
        org_id=seller_org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        owner_user_id=seller.id,
    )
    legacy_payload = {"order_id": str(order.id), "quantity_mt": "100.00"}
    existing = _pending_trade(
        order,
        buyer_id=buyer_org.id,
        seller_id=seller_org.id,
        quantity="100.00",
    )
    existing.buyer_user_id = buyer.id
    existing.seller_user_id = seller.id
    existing.idempotency_key = "old-success"
    existing.idempotency_operation = TRADE_CREATE_OPERATION
    existing.idempotency_request_hash = idempotency_request_hash(legacy_payload)
    order.remaining_quantity_mt = Decimal("900")
    order.price_per_mt_usd += Decimal("50")
    db.add(existing)
    await db.commit()
    request = _fake_request()
    request.headers["Idempotency-Key"] = existing.idempotency_key

    response = await trades.create_trade(
        payload=trades.TradeCreate(**legacy_payload),
        request=request,
        db=db,
        current_user=buyer,
    )

    assert response.id == existing.id
    assert response.price_per_mt_usd == existing.price_per_mt_usd
    assert await db.scalar(select(func.count()).select_from(Trade)) == 1


def test_new_digest_is_part_of_idempotency_hash():
    legacy = trades.TradeCreate(
        order_id="00000000-0000-0000-0000-000000000001",
        quantity_mt="100.00",
    )
    assert trades.trade_create_idempotency_payload(legacy) == {
        "order_id": "00000000-0000-0000-0000-000000000001",
        "quantity_mt": "100.00",
    }
    reviewed = legacy.model_copy(update={"expected_terms_digest": "a" * 64})
    assert idempotency_request_hash(trades.trade_create_idempotency_payload(reviewed)) != (
        idempotency_request_hash(trades.trade_create_idempotency_payload(legacy))
    )
