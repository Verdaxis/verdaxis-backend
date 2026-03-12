"""Activity event publishers and price alert checker."""
from datetime import datetime, UTC
from decimal import Decimal
from typing import Optional
from uuid import UUID

from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select

from app.services.event_bus import event_bus


async def publish_new_listing(order, product, delivery_point) -> None:
    """Publish a new-listing event to the public activity channel."""
    await event_bus.publish(
        "activity",
        "new_listing",
        {
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
