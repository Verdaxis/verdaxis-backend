"""Default B100 fields must not invalidate a pre-deployment trade replay."""

from decimal import Decimal
from uuid import UUID

import pytest
from sqlalchemy import func, select

from app.models.orderbook import Trade
from app.models.user import OrgType, UserRole
from app.routers import trades
from app.services.idempotency import TRADE_CREATE_OPERATION, idempotency_request_hash
from tests.unit.test_fame_manual_trade_routes import (
    fuel_pair as fuel_pair,  # noqa: PLC0414 -- shared pytest fixture
)
from tests.unit.test_trade_watchlist_hooks import (
    _fake_request,
    _make_ask,
    _make_delivery_point,
    _make_org,
    _make_product,
    _make_user,
    _pending_trade,
    async_engine,  # noqa: F401 -- shared pytest fixtures
    setup_tables,  # noqa: F401
)
from tests.unit.test_trade_watchlist_hooks import (
    db as db,  # noqa: PLC0414 -- shared pytest fixture
)


@pytest.mark.parametrize("explicit_defaults", [False, True])
def test_legacy_trade_hash_is_unchanged_by_new_default_fields(explicit_defaults):
    legacy = {
        "order_id": "00000000-0000-0000-0000-000000000001",
        "quantity_mt": "100.00",
    }
    extras = (
        {
            "fame_terms": None,
            "certification_declared": False,
            "msds_available": False,
            "expected_order_version": None,
        }
        if explicit_defaults
        else {}
    )
    payload = trades.TradeCreate(**legacy, **extras)
    canonical = trades.trade_create_idempotency_payload(payload)
    assert canonical == legacy
    assert (
        idempotency_request_hash(canonical)
        == "50100b9c12a7adcda6a9a73cb7b4ea30e7d78f054295b71ba79231e95a3ffa19"
    )


def test_b100_terms_and_true_acknowledgements_remain_in_trade_hash(fuel_pair):
    payload = trades.TradeCreate(
        order_id=UUID("00000000-0000-0000-0000-000000000001"),
        quantity_mt="100.00",
        fame_terms=fuel_pair[1],
        certification_declared=True,
        msds_available=True,
        expected_order_version=1,
    )
    canonical = trades.trade_create_idempotency_payload(payload)
    assert canonical == payload.model_dump(mode="json")
    original_hash = idempotency_request_hash(canonical)
    for change in (
        {"certification_declared": False},
        {"msds_available": False},
        {"expected_order_version": 2},
        {"fame_terms": fuel_pair[1].model_copy(update={"cfpp_c": Decimal(-20)})},
    ):
        revised = payload.model_copy(update=change)
        assert (
            idempotency_request_hash(trades.trade_create_idempotency_payload(revised))
            != original_hash
        )


@pytest.mark.asyncio
async def test_pre_deployment_alcohol_trade_replays_without_consuming_capacity(db):
    buyer_org = await _make_org(db, "Legacy buyer", OrgType.SHIPPING_LINE)
    seller_org = await _make_org(db, "Legacy seller", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    seller = await _make_user(db, seller_org, UserRole.SUPPLIER)
    product = await _make_product(db)
    point = await _make_delivery_point(db)
    ask = await _make_ask(
        db,
        org_id=seller_org.id,
        product_id=product.id,
        delivery_point_id=point.id,
        owner_user_id=seller.id,
    )
    legacy_payload = {"order_id": str(ask.id), "quantity_mt": "100.00"}
    existing = _pending_trade(
        ask, buyer_id=buyer_org.id, seller_id=seller_org.id, quantity="100.00"
    )
    existing.buyer_user_id = buyer.id
    existing.seller_user_id = seller.id
    existing.idempotency_key = "pre-deployment-trade"
    existing.idempotency_operation = TRADE_CREATE_OPERATION
    existing.idempotency_request_hash = idempotency_request_hash(legacy_payload)
    ask.remaining_quantity_mt -= Decimal(100)
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
    assert ask.remaining_quantity_mt == Decimal(900)
    assert await db.scalar(select(func.count()).select_from(Trade)) == 1
