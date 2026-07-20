"""Activity event publishers and price alert checker."""
from datetime import datetime, UTC
from decimal import Decimal
from typing import Optional
from uuid import UUID

from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.models.orderbook import TradeStatus
from app.services.availability_windows import normalize_availability_window
from app.services.demo_market import is_demo_market_organization
from app.services.event_bus import event_bus


async def publish_trade_event(trade, event_type: str, data: dict) -> None:
    """Publish lifecycle data only to the two participating org channels."""
    channels = {f"trades:{trade.buyer_id}", f"trades:{trade.seller_id}"}
    for channel in channels:
        await event_bus.publish(channel, event_type, data)


def order_activity_provenance(order) -> dict:
    is_demo = is_demo_market_organization(getattr(order, "organization_id", None))
    observed_at = getattr(order, "updated_at", None) or getattr(order, "created_at", None)
    delivery_point_id = getattr(order, "delivery_point_id", None)
    if not isinstance(delivery_point_id, UUID):
        delivery_point_id = None
    market_product = getattr(order, "market_product", None)
    if not isinstance(market_product, str):
        market_product = None
    delivery_point_name = getattr(order, "delivery_point_name", None)
    if not isinstance(delivery_point_name, str):
        delivery_point_name = None
    availability_window = getattr(order, "availability_window", None)
    normalized_window = None
    if isinstance(availability_window, str) and availability_window:
        normalized_window = normalize_availability_window(availability_window)
    return {
        "source_kind": MarketSourceKind.DEMO_SEED.value if is_demo else MarketSourceKind.LIVE_ORDER.value,
        "demo_status": MarketDemoStatus.DEMO_ONLY.value if is_demo else MarketDemoStatus.REAL_ONLY.value,
        "scope": MarketScope.DELIVERY_POINT.value if delivery_point_id else MarketScope.UNKNOWN.value,
        "observed_at": observed_at.isoformat() if observed_at else None,
        "market_product": market_product,
        "delivery_point_id": str(delivery_point_id) if delivery_point_id else None,
        "delivery_point": delivery_point_name,
        "availability_window": normalized_window,
    }


def trade_activity_provenance(trade) -> dict:
    buyer_demo = is_demo_market_organization(getattr(trade, "buyer_id", None))
    seller_demo = is_demo_market_organization(getattr(trade, "seller_id", None))
    if buyer_demo and seller_demo:
        source_kind = MarketSourceKind.DEMO_SEED
        demo_status = MarketDemoStatus.DEMO_ONLY
    elif buyer_demo or seller_demo:
        source_kind = MarketSourceKind.UNKNOWN
        demo_status = MarketDemoStatus.UNKNOWN
    else:
        source_kind = MarketSourceKind.CONFIRMED_TRADE
        demo_status = MarketDemoStatus.REAL_ONLY
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
    order = getattr(trade, "ask_order", None) or getattr(trade, "bid_order", None)
    payload = {
        "source_kind": source_kind.value,
        "demo_status": demo_status.value,
        "scope": MarketScope.DELIVERY_POINT.value if order and getattr(order, "delivery_point_id", None) else MarketScope.UNKNOWN.value,
        "observed_at": observed_at.isoformat() if observed_at else None,
    }
    if order is not None:
        delivery_point_id = getattr(order, "delivery_point_id", None)
        if not isinstance(delivery_point_id, UUID):
            delivery_point_id = None
        market_product = getattr(order, "market_product", None)
        if not isinstance(market_product, str):
            market_product = None
        delivery_point_name = getattr(order, "delivery_point_name", None)
        if not isinstance(delivery_point_name, str):
            delivery_point_name = None
        availability_window = getattr(order, "availability_window", None)
        normalized_window = normalize_availability_window(availability_window) if isinstance(availability_window, str) and availability_window else None
        payload.update({
            "market_product": market_product,
            "delivery_point_id": str(delivery_point_id) if delivery_point_id else None,
            "delivery_point": delivery_point_name,
            "availability_window": normalized_window,
        })
    return payload


async def publish_new_listing(order, product, delivery_point) -> None:
    """Publish a new-listing event to the public activity channel."""
    await event_bus.publish(
        "activity",
        "new_listing",
        {
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


async def publish_price_crossing(product, delivery_point) -> None:
    """Publish a bid/ask price-crossing event to the public activity channel."""
    await event_bus.publish(
        "activity",
        "price_crossing",
        {
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


async def publish_order_outbid(org_id, order, new_price: Decimal) -> None:
    """Publish an outbid notification to the participant-specific activity channel."""
    await event_bus.publish(
        f"activity:{org_id}",
        "order_outbid",
        {
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
) -> None:
    """Query active alerts for the product/dp and trigger any that cross the threshold.

    Triggered alerts are deactivated (is_active=False) and stamped with triggered_at.
    A participant-specific SSE event is emitted for each triggered alert.
    """
    from app.models.alerts import PriceAlert

    query = select(PriceAlert).where(
        PriceAlert.product_id == product_id,
        PriceAlert.is_active == True,
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
        return

    now = datetime.now(UTC)
    for alert in triggered:
        alert.triggered_at = now
        alert.is_active = False
        await event_bus.publish(
            f"activity:{alert.org_id}",
            "price_alert_triggered",
            {
                "alert_id": str(alert.id),
                "product_id": str(alert.product_id),
                "direction": alert.direction,
                "threshold_usd": str(alert.threshold_usd),
                "triggered_price": str(price),
            },
        )

    await db.commit()
