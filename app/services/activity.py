"""Build market activity events without owning commit or publication."""
from datetime import datetime, UTC
from decimal import Decimal
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.models.orderbook import TradeStatus
from app.services.event_bus import event_bus
from app.services.market_events import CommittedMarketEvent, participant_market_event
from app.services.market_provenance import order_market_provenance, trade_market_provenance


async def publish_trade_event(trade, event_type: str, data: dict) -> None:
    """Publish lifecycle data only to the two participating org channels."""
    channels = {f"trades:{trade.buyer_id}", f"trades:{trade.seller_id}"}
    for channel in channels:
        await event_bus.publish(channel, event_type, data)


def order_activity_provenance(order) -> dict:
    policy = order_market_provenance(order)
    observed_at = getattr(order, "updated_at", None) or getattr(order, "created_at", None)
    delivery_point_name = getattr(order, "delivery_point_name", None)
    return {
        **policy,
        "observed_at": observed_at.isoformat() if observed_at else None,
        "delivery_point": delivery_point_name,
    }


def trade_activity_provenance(trade) -> dict:
    policy = trade_market_provenance(trade)
    status = getattr(trade, "status", None)
    status_value = status.value if hasattr(status, "value") else status
    if status_value == TradeStatus.PAID.value:
        observed_at = getattr(trade, "paid_at", None) or getattr(trade, "delivered_at", None) or getattr(trade, "confirmed_at", None) or getattr(trade, "created_at", None)
    elif status_value == TradeStatus.DELIVERED.value:
        observed_at = getattr(trade, "delivered_at", None) or getattr(trade, "confirmed_at", None) or getattr(trade, "created_at", None)
    elif status_value == TradeStatus.CONFIRMED.value:
        observed_at = getattr(trade, "confirmed_at", None) or getattr(trade, "created_at", None)
    else:
        observed_at = getattr(trade, "created_at", None)
    payload = {**policy, "observed_at": observed_at.isoformat() if observed_at else None}
    return payload


def new_listing_event(order, product, delivery_point) -> CommittedMarketEvent:
    """Build a durable event scoped to the listing owner."""
    return participant_market_event(
        event_type="new_listing",
        aggregate_type="order",
        aggregate_id=order.id,
        participant_org_ids=(order.organization_id,),
        payload={
            **order_activity_provenance(order),
            "order_id": str(order.id),
            "side": order.side.value if hasattr(order.side, "value") else order.side,
            "product_id": str(product.id),
            "product_name": product.name,
            "fuel_type": product.fuel_type,
            "delivery_point": delivery_point.name if delivery_point else None,
            "delivery_point_id": str(delivery_point.id) if delivery_point else None,
            "price": str(order.price_per_mt_usd),
            "quantity": str(order.remaining_quantity_mt),
        },
    )


def price_crossing_event(
    product,
    delivery_point,
    *,
    participant_org_ids,
) -> CommittedMarketEvent:
    """Build a price-crossing event for explicitly named participants."""
    return participant_market_event(
        event_type="price_crossing",
        aggregate_type="product",
        aggregate_id=product.id,
        participant_org_ids=participant_org_ids,
        payload={
            "product_id": str(product.id),
            "product_name": product.name,
            "fuel_type": product.fuel_type,
            "delivery_point": delivery_point.name if delivery_point else None,
            "delivery_point_id": str(delivery_point.id) if delivery_point else None,
            "source_kind": MarketSourceKind.UNKNOWN.value,
            "demo_status": MarketDemoStatus.UNKNOWN.value,
            "scope": MarketScope.DELIVERY_POINT.value if delivery_point else MarketScope.UNKNOWN.value,
            "observed_at": None,
        },
    )


def order_outbid_event(org_id, order, new_price: Decimal) -> CommittedMarketEvent:
    """Build an outbid event for publication only after commit."""
    return participant_market_event(
        event_type="order_outbid",
        aggregate_type="order",
        aggregate_id=order.id,
        participant_org_ids=(org_id,),
        payload={
            "order_id": str(order.id),
            "new_price": str(new_price),
            "your_price": str(order.price_per_mt_usd),
        },
    )


async def check_price_alerts(
    db: AsyncSession,
    product_id,
    delivery_point_id,
    price: Decimal,
) -> list[CommittedMarketEvent]:
    """Query active alerts for the product/dp and trigger any that cross the threshold.

    Triggered alerts are deactivated (is_active=False) and stamped with triggered_at.
    The caller owns commit and then publishes the returned event descriptors.
    """
    from app.models.alerts import PriceAlert

    query = select(PriceAlert).where(
        PriceAlert.product_id == product_id,
        PriceAlert.is_active.is_(True),
    )
    if delivery_point_id is not None:
        # Match alerts for this specific dp OR alerts watching all delivery points (NULL)
        from sqlalchemy import or_
        query = query.where(
            or_(
                PriceAlert.delivery_point_id == delivery_point_id,
                PriceAlert.delivery_point_id.is_(None),
            )
        )

    result = await db.execute(query)
    alerts = result.scalars().all()

    triggered = []
    for alert in alerts:
        if alert.direction == "above" and price > alert.threshold_usd:
            triggered.append(alert)
        elif alert.direction == "below" and price < alert.threshold_usd:
            triggered.append(alert)

    if not triggered:
        return []

    now = datetime.now(UTC)
    events: list[CommittedMarketEvent] = []
    for alert in triggered:
        alert.triggered_at = now
        alert.is_active = False
        events.append(participant_market_event(
            event_type="price_alert_triggered",
            aggregate_type="price_alert",
            aggregate_id=alert.id,
            participant_org_ids=(alert.org_id,),
            payload={
                "alert_id": str(alert.id),
                "product_id": str(alert.product_id),
                "direction": alert.direction,
                "threshold_usd": str(alert.threshold_usd),
                "triggered_price": str(price),
            },
        ))
    return events
