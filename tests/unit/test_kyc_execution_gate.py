"""Route-level proofs for the KYC execution gate (design section 7).

New executions return KYC_REQUIRED; cleanup never applies KYC; an exact
replay returns the stored result before the gate; assisted execution needs
one KYC-eligible customer member.
"""
from datetime import UTC, datetime
from decimal import Decimal
from types import SimpleNamespace
from uuid import uuid4

import pytest
from fastapi import HTTPException
from sqlalchemy.ext.asyncio import AsyncSession

from app.middleware.execution import require_execution_eligible_user
from app.models.orderbook import OrderCreationMethod, OrderSide, TradeStatus
from app.models.subscription import Subscription, SubscriptionTier
from app.models.user import Organization, OrganizationProvenance, OrgType, User, UserRole
from app.routers import inventory as inventory_router
from app.routers import orderbook as orderbook_router
from app.routers import trades as trades_router
from app.schemas.orderbook import OrderCreate, OrderUpdate, TradeDeliverPayload
from app.services.execution_policy import MarketPartyPolicy, order_owner_is_execution_eligible
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

KYC_REQUIRED_DETAIL = {
    "code": "KYC_REQUIRED",
    "message": "Complete KYC verification to trade.",
    "kyc_status": "PENDING",
    "grace_expires_at": None,
}


async def _without_kyc(db: AsyncSession, user: User) -> User:
    user.kyc_status = "PENDING"
    user.kyc_organization_id = None
    await db.commit()
    return user


async def _market(db: AsyncSession, monkeypatch):
    """A buyer, a supplier with one resting ASK, and silent notifications."""
    async def _noop_notify(*args, **kwargs):
        return None

    monkeypatch.setattr(trades_router, "notify_org_users", _noop_notify)
    buyer_org = await _make_org(db, "Buyer", OrgType.SHIPPING_LINE)
    supplier_org = await _make_org(db, "Supplier", OrgType.FUEL_SUPPLIER)
    buyer = await _make_user(db, buyer_org, UserRole.BUYER)
    supplier = await _make_user(db, supplier_org, UserRole.SUPPLIER)
    ask = await _make_ask(
        db,
        org_id=supplier_org.id,
        product_id=(await _make_product(db)).id,
        delivery_point_id=(await _make_delivery_point(db)).id,
        owner_user_id=supplier.id,
    )
    db.add(Subscription(org_id=supplier_org.id, tier=SubscriptionTier.STANDARD, started_at=datetime.now(UTC)))
    await db.commit()
    return SimpleNamespace(buyer=buyer, supplier=supplier, ask=ask)


async def _trade(db: AsyncSession, market, *, status: TradeStatus):
    trade = _pending_trade(
        market.ask,
        buyer_id=market.buyer.organization_id,
        seller_id=market.supplier.organization_id,
        quantity="100",
    )
    trade.buyer_user_id = market.buyer.id
    trade.seller_user_id = market.supplier.id
    trade.status = status
    db.add(trade)
    await db.commit()
    return trade


def _request(**headers: str):
    return SimpleNamespace(
        headers=headers, method="DELETE", client=SimpleNamespace(host="127.0.0.1")
    )


async def _expect_kyc_required(call) -> None:
    with pytest.raises(HTTPException) as failure:
        await call
    assert failure.value.status_code == 403
    assert failure.value.detail == KYC_REQUIRED_DETAIL


@pytest.mark.asyncio
async def test_new_executions_return_kyc_required(db, monkeypatch):
    market = await _market(db, monkeypatch)
    pending_trade = await _trade(db, market, status=TradeStatus.PENDING_CONFIRMATION)
    buyer = await _without_kyc(db, market.buyer)
    supplier = await _without_kyc(db, market.supplier)

    await _expect_kyc_required(
        orderbook_router.create_order(
            request=_request(),
            order_data=OrderCreate(
                side=OrderSide.BID,
                product_id=market.ask.product_id,
                delivery_point_id=market.ask.delivery_point_id,
                quantity_mt=Decimal("500"),
                price_per_mt_usd=Decimal("1000"),
            ),
            current_user=buyer,
            db=db,
        )
    )
    await _expect_kyc_required(
        orderbook_router.update_order(
            order_id=market.ask.id,
            request=_request(),
            update_data=OrderUpdate(price_per_mt_usd=Decimal("1080")),
            current_user=supplier,
            db=db,
        )
    )
    await _expect_kyc_required(
        trades_router.create_trade(
            payload=trades_router.TradeCreate(
                order_id=market.ask.id,
                quantity_mt=Decimal("100"),
                expected_terms_digest=order_terms_digest(market.ask),
            ),
            request=_request(),
            db=db,
            current_user=buyer,
        )
    )
    await _expect_kyc_required(
        trades_router.confirm_trade(
            trade_id=pending_trade.id, request=_request(), db=db, current_user=supplier
        )
    )
    # Inventory publication is gated by its route dependency.
    publish_route = next(
        route
        for route in inventory_router.router.routes
        if route.path == "/inventory/{item_id}/publish"
    )
    assert require_execution_eligible_user in {
        dependency.call for dependency in publish_route.dependant.dependencies
    }
    await _expect_kyc_required(require_execution_eligible_user(current_user=supplier, db=db))


@pytest.mark.asyncio
async def test_cleanup_actions_never_apply_kyc(db, monkeypatch):
    market = await _market(db, monkeypatch)
    pending_trade = await _trade(db, market, status=TradeStatus.PENDING_CONFIRMATION)
    confirmed_trade = await _trade(db, market, status=TradeStatus.CONFIRMED)
    delivered_trade = await _trade(db, market, status=TradeStatus.DELIVERED)
    buyer = await _without_kyc(db, market.buyer)
    supplier = await _without_kyc(db, market.supplier)

    declined = await trades_router.decline_trade(
        trade_id=pending_trade.id, request=_request(), db=db, current_user=supplier
    )
    delivered = await trades_router.deliver_trade(
        trade_id=confirmed_trade.id,
        payload=TradeDeliverPayload(
            final_quantity_mt=Decimal("100"), final_price_per_mt=Decimal("1090")
        ),
        request=_request(),
        db=db,
        current_user=buyer,
    )
    paid = await trades_router.pay_trade(
        trade_id=delivered_trade.id, request=_request(), db=db, current_user=supplier
    )
    cancelled = await orderbook_router.cancel_order(
        order_id=market.ask.id,
        request=_request(),
        current_user=supplier,
        db=db,
        if_match=None,
        body=None,
    )

    assert declined.status == TradeStatus.DECLINED
    assert delivered.status == TradeStatus.DELIVERED
    assert paid.status == TradeStatus.PAID
    assert cancelled.status_code == 204


@pytest.mark.asyncio
async def test_exact_replay_returns_the_stored_trade_before_the_gate(db, monkeypatch):
    market = await _market(db, monkeypatch)
    payload = trades_router.TradeCreate(
        order_id=market.ask.id,
        quantity_mt=Decimal("100"),
        expected_terms_digest=order_terms_digest(market.ask),
    )
    created = await trades_router.create_trade(
        payload=payload,
        request=_request(**{"Idempotency-Key": "kyc-replay-1"}),
        db=db,
        current_user=market.buyer,
    )
    buyer = await _without_kyc(db, market.buyer)

    replayed = await trades_router.create_trade(
        payload=payload,
        request=_request(**{"Idempotency-Key": "kyc-replay-1"}),
        db=db,
        current_user=buyer,
    )

    assert replayed.id == created.id
    # A new execution under a new key is gated.
    await _expect_kyc_required(
        trades_router.create_trade(
            payload=payload,
            request=_request(**{"Idempotency-Key": f"kyc-replay-{uuid4()}"}),
            db=db,
            current_user=buyer,
        )
    )


@pytest.mark.asyncio
async def test_assisted_execution_needs_one_kyc_eligible_customer_member(db, monkeypatch):
    market = await _market(db, monkeypatch)
    customer_org = await db.get(Organization, market.supplier.organization_id)
    customer_org.provenance = OrganizationProvenance.REAL
    support_admin = await _make_user(db, await _make_org(db, "Support", OrgType.FUEL_TRADER), UserRole.ADMIN)
    assisted_ask = await _make_ask(
        db,
        org_id=customer_org.id,
        product_id=market.ask.product_id,
        delivery_point_id=market.ask.delivery_point_id,
        owner_user_id=support_admin.id,
    )
    assisted_ask.creation_method = OrderCreationMethod.MARKET_SUPPORT
    assisted_ask.created_by_actor_user_id = support_admin.id
    await db.commit()

    async def assisted_owner_allowed(policy=MarketPartyPolicy.EXECUTION) -> bool:
        return await order_owner_is_execution_eligible(
            db, order=assisted_ask, user=support_admin, organization=customer_org, policy=policy
        )

    # The supplier is the customer's only member and is KYC-approved.
    assert await assisted_owner_allowed()
    await _without_kyc(db, market.supplier)
    assert not await assisted_owner_allowed()
    # Settling an existing assisted trade never applies KYC.
    assert await assisted_owner_allowed(MarketPartyPolicy.CLEANUP)
