"""PostgreSQL authority for narrow user/org executable-state invalidation."""

from datetime import UTC, datetime, timedelta
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import select

from app.models.catalog import Product
from app.models.negotiation import Negotiation, NegotiationStatus
from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.models.rfq import QuoteStatus, RFQ, RFQQuote, RFQStatus
from app.models.user import Organization, OrgType, User, UserRole, UserStatus
from app.services.execution_invalidation import invalidate_execution_state


async def _seed_identity(session, label: str, org: Organization) -> User:
    user = User(
        id=uuid4(),
        email=f"{label}-{uuid4()}@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        organization_id=org.id,
        email_verified=True,
    )
    session.add(user)
    await session.flush()
    return user


async def _seed_bundle(session, *, user: User, org: Organization, product: Product,
                       neutral_user: User, neutral_org: Organization) -> dict[str, object]:
    order = OrderBookOrder(
        organization_id=org.id,
        owner_user_id=user.id,
        side=OrderSide.BID,
        product_id=product.id,
        quantity_mt=Decimal("10"),
        remaining_quantity_mt=Decimal("10"),
        price_per_mt_usd=Decimal("100"),
        status=OrderBookStatus.OPEN,
    )
    rfq = RFQ(
        buyer_org_id=org.id,
        buyer_user_id=user.id,
        product_id=product.id,
        quantity_mt=Decimal("10"),
        availability_window="SPOT",
        status=RFQStatus.OPEN,
        expires_at=datetime.now(UTC) + timedelta(days=1),
    )
    session.add_all([order, rfq])
    await session.flush()
    quote = RFQQuote(
        rfq_id=rfq.id,
        seller_org_id=org.id,
        seller_user_id=user.id,
        price_per_mt_usd=Decimal("101"),
        status=QuoteStatus.PENDING,
    )
    negotiation = Negotiation(
        initiator_org_id=org.id,
        counterparty_org_id=neutral_org.id,
        initiator_user_id=user.id,
        counterparty_user_id=neutral_user.id,
        initiator_side="BUYER",
        product_id=product.id,
        quantity_mt=Decimal("10"),
        current_price=Decimal("100"),
        status=NegotiationStatus.OPEN,
        last_actor_org_id=org.id,
        expires_at=datetime.now(UTC) + timedelta(days=1),
    )
    session.add_all([quote, negotiation])
    await session.flush()
    return {
        "order": order.id,
        "rfq": rfq.id,
        "quote": quote.id,
        "negotiation": negotiation.id,
    }


async def _seed_scenario(session):
    target_org = Organization(id=uuid4(), name=f"Target {uuid4()}", type=OrgType.FUEL_BUYER)
    other_org = Organization(id=uuid4(), name=f"Other {uuid4()}", type=OrgType.FUEL_BUYER)
    neutral_org = Organization(id=uuid4(), name=f"Neutral {uuid4()}", type=OrgType.FUEL_BUYER)
    product = Product(
        id=uuid4(),
        name=f"Product {uuid4()}",
        fuel_type="Methanol",
        fuel_grade="Bio",
    )
    session.add_all([target_org, other_org, neutral_org, product])
    await session.flush()
    target = await _seed_identity(session, "target", target_org)
    colleague = await _seed_identity(session, "colleague", target_org)
    outsider = await _seed_identity(session, "outsider", other_org)
    neutral = await _seed_identity(session, "neutral", neutral_org)
    bundles = {
        "target": await _seed_bundle(
            session, user=target, org=target_org, product=product,
            neutral_user=neutral, neutral_org=neutral_org,
        ),
        "colleague": await _seed_bundle(
            session, user=colleague, org=target_org, product=product,
            neutral_user=neutral, neutral_org=neutral_org,
        ),
        "outsider": await _seed_bundle(
            session, user=outsider, org=other_org, product=product,
            neutral_user=neutral, neutral_org=neutral_org,
        ),
    }
    await session.commit()
    return target_org, other_org, target, bundles


async def _bundle_statuses(session, bundle: dict[str, object]) -> tuple[object, ...]:
    return (
        await session.scalar(select(OrderBookOrder.status).where(OrderBookOrder.id == bundle["order"])),
        await session.scalar(select(RFQ.status).where(RFQ.id == bundle["rfq"])),
        await session.scalar(select(RFQQuote.status).where(RFQQuote.id == bundle["quote"])),
        await session.scalar(select(Negotiation.status).where(Negotiation.id == bundle["negotiation"])),
    )


_ACTIVE = (OrderBookStatus.OPEN, RFQStatus.OPEN, QuoteStatus.PENDING, NegotiationStatus.OPEN)
_INVALIDATED = (
    OrderBookStatus.CANCELLED,
    RFQStatus.CANCELLED,
    QuoteStatus.WITHDRAWN,
    NegotiationStatus.DECLINED,
)


async def test_user_only_invalidation_does_not_cancel_colleagues_state(pg_session):
    _engine, session = pg_session
    _target_org, _other_org, target, bundles = await _seed_scenario(session)

    counts = await invalidate_execution_state(session, user_ids=[target.id])
    await session.commit()

    assert counts == {
        "orders_cancelled": 1,
        "rfqs_cancelled": 1,
        "quotes_withdrawn": 1,
        "negotiations_declined": 1,
    }
    assert await _bundle_statuses(session, bundles["target"]) == _INVALIDATED
    assert await _bundle_statuses(session, bundles["colleague"]) == _ACTIVE
    assert await _bundle_statuses(session, bundles["outsider"]) == _ACTIVE


async def test_org_only_invalidation_cancels_every_member_not_other_orgs(pg_session):
    _engine, session = pg_session
    target_org, _other_org, _target, bundles = await _seed_scenario(session)

    await invalidate_execution_state(session, organization_ids=[target_org.id])
    await session.commit()

    assert await _bundle_statuses(session, bundles["target"]) == _INVALIDATED
    assert await _bundle_statuses(session, bundles["colleague"]) == _INVALIDATED
    assert await _bundle_statuses(session, bundles["outsider"]) == _ACTIVE


async def test_combined_invalidation_is_union_not_intersection_or_org_expansion(pg_session):
    _engine, session = pg_session
    _target_org, other_org, target, bundles = await _seed_scenario(session)

    await invalidate_execution_state(
        session,
        user_ids=[target.id],
        organization_ids=[other_org.id],
    )
    await session.commit()

    assert await _bundle_statuses(session, bundles["target"]) == _INVALIDATED
    assert await _bundle_statuses(session, bundles["outsider"]) == _INVALIDATED
    assert await _bundle_statuses(session, bundles["colleague"]) == _ACTIVE
