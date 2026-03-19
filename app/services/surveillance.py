"""Surveillance Engine — market manipulation detection service.

Detectors implemented:
  - Wash Trading   (S6-002): buyer_id == seller_id on the same trade
  - Spoofing       (S6-003): rapid place-then-cancel patterns
  - Front-Running  (S6-004): small order filled just before large order
  - Layering       (S6-004): multiple price-level orders, >50% cancelled
  - Marking the Close (S6-004): orders placed in last N minutes before market close
"""
from __future__ import annotations

from datetime import datetime, UTC, timedelta
from typing import Optional

from sqlalchemy import select, func, distinct, or_
from sqlalchemy.orm import aliased
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.orderbook import OrderBookOrder, OrderBookStatus, Trade
from app.models.surveillance import (
    SurveillanceEvent,
    SurveillanceType,
    SurveillanceSeverity,
    SurveillanceStatus,
)


class SurveillanceEngine:
    def __init__(self, db: AsyncSession) -> None:
        self.db = db

    # -----------------------------------------------------------------------
    # S6-002: Wash Trading
    # -----------------------------------------------------------------------

    async def check_wash_trading(self, trade: Trade) -> Optional[SurveillanceEvent]:
        """Detect trades where buyer_id == seller_id (same org on both sides)."""
        if trade.buyer_id != trade.seller_id:
            return None

        event = SurveillanceEvent(
            type=SurveillanceType.WASH_TRADING,
            severity=SurveillanceSeverity.HIGH,
            status=SurveillanceStatus.OPEN,
            participants=[str(trade.buyer_id)],
            related_trades=[str(trade.id)],
            description=f"Self-trade detected: org {trade.buyer_id} on both sides of trade {trade.id}",
            auto_detected=True,
            created_at=datetime.now(UTC),
            updated_at=datetime.now(UTC),
        )
        self.db.add(event)
        return event

    # -----------------------------------------------------------------------
    # S6-003: Spoofing
    # -----------------------------------------------------------------------

    async def check_spoofing(
        self,
        cancelled_order: OrderBookOrder,
        window_seconds: int = 60,
        threshold: int = 3,
    ) -> Optional[SurveillanceEvent]:
        """Detect rapid place-then-cancel patterns from the same org.

        Counts CANCELLED orders for the same (org, fuel_type) whose updated_at
        falls within the look-back window. If count >= threshold an event is raised.
        """
        cutoff = datetime.now(UTC) - timedelta(seconds=window_seconds)

        stmt = (
            select(func.count())
            .select_from(OrderBookOrder)
            .where(
                OrderBookOrder.id != cancelled_order.id,  # Exclude the triggering order itself
                OrderBookOrder.organization_id == cancelled_order.organization_id,
                OrderBookOrder.fuel_type == cancelled_order.fuel_type,
                OrderBookOrder.status == OrderBookStatus.CANCELLED,
                OrderBookOrder.created_at >= cutoff,
            )
        )
        result = await self.db.execute(stmt)
        cancel_count = result.scalar() or 0

        if cancel_count < threshold:
            return None

        event = SurveillanceEvent(
            type=SurveillanceType.SPOOFING,
            severity=SurveillanceSeverity.MEDIUM,
            status=SurveillanceStatus.OPEN,
            participants=[str(cancelled_order.organization_id)],
            related_orders=[str(cancelled_order.id)],
            description=(
                f"Spoofing pattern: org {cancelled_order.organization_id} cancelled "
                f"{cancel_count} {cancelled_order.fuel_type} orders within {window_seconds}s"
            ),
            window_start=cutoff,
            window_end=datetime.now(UTC),
            auto_detected=True,
            created_at=datetime.now(UTC),
            updated_at=datetime.now(UTC),
        )
        self.db.add(event)
        return event

    # -----------------------------------------------------------------------
    # S6-004: Front-Running
    # -----------------------------------------------------------------------

    async def check_front_running(
        self,
        trade: Trade,
        window_seconds: int = 60,
        size_ratio_threshold: int = 10,
    ) -> Optional[SurveillanceEvent]:
        """Detect small orders filled just before a large order by a different counterparty.

        Looks for trades by a *different* seller that completed in the window
        immediately before this trade and whose quantity is at least
        size_ratio_threshold times smaller than this trade.
        """
        if trade.created_at is None:
            return None

        cutoff = trade.created_at - timedelta(seconds=window_seconds)

        # Determine the fuel_type of the triggering trade via its linked order (if any)
        TriggerOrder = aliased(OrderBookOrder)
        fuel_type_stmt = (
            select(TriggerOrder.fuel_type)
            .where(
                or_(
                    TriggerOrder.id == trade.bid_order_id,
                    TriggerOrder.id == trade.ask_order_id,
                )
            )
            .limit(1)
        )
        ft_result = await self.db.execute(fuel_type_stmt)
        trade_fuel_type = ft_result.scalar_one_or_none()

        # Preceding trades — filter by fuel_type when we can determine it
        PrecedingOrder = aliased(OrderBookOrder)
        stmt = (
            select(Trade)
            .outerjoin(
                PrecedingOrder,
                or_(
                    PrecedingOrder.id == Trade.bid_order_id,
                    PrecedingOrder.id == Trade.ask_order_id,
                ),
            )
            .where(
                Trade.seller_id != trade.seller_id,
                Trade.created_at >= cutoff,
                Trade.created_at < trade.created_at,
            )
        )
        if trade_fuel_type is not None:
            stmt = stmt.where(PrecedingOrder.fuel_type == trade_fuel_type)

        result = await self.db.execute(stmt)
        preceding_trades = result.scalars().all()

        suspicious = [
            t for t in preceding_trades
            if t.quantity_mt > 0 and trade.quantity_mt / t.quantity_mt >= size_ratio_threshold
        ]

        if not suspicious:
            return None

        participant_ids = list({str(t.seller_id) for t in suspicious})
        related_trade_ids = [str(t.id) for t in suspicious] + [str(trade.id)]

        event = SurveillanceEvent(
            type=SurveillanceType.FRONT_RUNNING,
            severity=SurveillanceSeverity.HIGH,
            status=SurveillanceStatus.OPEN,
            participants=participant_ids,
            related_trades=related_trade_ids,
            description=(
                f"Potential front-running: {len(suspicious)} small trade(s) executed "
                f"within {window_seconds}s before large trade {trade.id} "
                f"(qty={trade.quantity_mt})"
            ),
            window_start=cutoff,
            window_end=trade.created_at,
            auto_detected=True,
            created_at=datetime.now(UTC),
            updated_at=datetime.now(UTC),
        )
        self.db.add(event)
        return event

    # -----------------------------------------------------------------------
    # S6-004: Layering
    # -----------------------------------------------------------------------

    async def check_layering(
        self,
        cancelled_order: OrderBookOrder,
        session_hours: int = 8,
        cancel_rate_threshold: float = 0.5,
        min_price_levels: int = 3,
    ) -> Optional[SurveillanceEvent]:
        """Detect layering: multiple orders at different price levels, >50% cancelled.

        Queries the current trading session (session_hours back) for all orders
        from the same (org, fuel_type). Flags if the cancel rate exceeds the
        threshold AND there are >= min_price_levels distinct prices.
        """
        session_start = datetime.now(UTC) - timedelta(hours=session_hours)

        # Total orders in session
        total_stmt = (
            select(func.count())
            .select_from(OrderBookOrder)
            .where(
                OrderBookOrder.organization_id == cancelled_order.organization_id,
                OrderBookOrder.fuel_type == cancelled_order.fuel_type,
                OrderBookOrder.created_at >= session_start,
            )
        )
        total_result = await self.db.execute(total_stmt)
        total_count = total_result.scalar() or 0

        if total_count == 0:
            return None

        # Cancelled orders in session
        cancel_stmt = (
            select(func.count())
            .select_from(OrderBookOrder)
            .where(
                OrderBookOrder.organization_id == cancelled_order.organization_id,
                OrderBookOrder.fuel_type == cancelled_order.fuel_type,
                OrderBookOrder.status == OrderBookStatus.CANCELLED,
                OrderBookOrder.created_at >= session_start,
            )
        )
        cancel_result = await self.db.execute(cancel_stmt)
        cancel_count = cancel_result.scalar() or 0

        cancel_rate = cancel_count / total_count

        # Distinct price levels
        price_stmt = (
            select(func.count(distinct(OrderBookOrder.price_per_mt_usd)))
            .where(
                OrderBookOrder.organization_id == cancelled_order.organization_id,
                OrderBookOrder.fuel_type == cancelled_order.fuel_type,
                OrderBookOrder.created_at >= session_start,
            )
        )
        price_result = await self.db.execute(price_stmt)
        distinct_prices = price_result.scalar() or 0

        if cancel_rate <= cancel_rate_threshold or distinct_prices < min_price_levels:
            return None

        event = SurveillanceEvent(
            type=SurveillanceType.LAYERING,
            severity=SurveillanceSeverity.MEDIUM,
            status=SurveillanceStatus.OPEN,
            participants=[str(cancelled_order.organization_id)],
            related_orders=[str(cancelled_order.id)],
            description=(
                f"Layering pattern: org {cancelled_order.organization_id} placed orders at "
                f"{distinct_prices} distinct {cancelled_order.fuel_type} price levels, "
                f"{cancel_count}/{total_count} cancelled ({cancel_rate:.0%}) in session"
            ),
            window_start=session_start,
            window_end=datetime.now(UTC),
            auto_detected=True,
            created_at=datetime.now(UTC),
            updated_at=datetime.now(UTC),
        )
        self.db.add(event)
        return event

    # -----------------------------------------------------------------------
    # S6-004: Marking the Close
    # -----------------------------------------------------------------------

    async def check_marking_the_close(
        self,
        order: OrderBookOrder,
        market_close_hour: int = 17,
        window_minutes: int = 5,
    ) -> Optional[SurveillanceEvent]:
        """Detect orders placed in the last N minutes of the market session.

        Uses order.created_at to check proximity to market_close_hour (UTC).
        """
        order_time = order.created_at
        if order_time is None:
            return None

        # Construct today's close time in UTC
        close_time = order_time.replace(
            hour=market_close_hour, minute=0, second=0, microsecond=0
        )
        window_start = close_time - timedelta(minutes=window_minutes)

        if not (window_start <= order_time < close_time):
            return None

        event = SurveillanceEvent(
            type=SurveillanceType.MARKING_THE_CLOSE,
            severity=SurveillanceSeverity.LOW,
            status=SurveillanceStatus.OPEN,
            participants=[str(order.organization_id)],
            related_orders=[str(order.id)],
            description=(
                f"Order {order.id} placed at {order_time.strftime('%H:%M:%S')} UTC, "
                f"within {window_minutes} minutes of market close ({market_close_hour:02d}:00 UTC)"
            ),
            window_start=window_start,
            window_end=close_time,
            auto_detected=True,
            created_at=datetime.now(UTC),
            updated_at=datetime.now(UTC),
        )
        self.db.add(event)
        return event

    # -----------------------------------------------------------------------
    # S6-004: Composite check methods
    # -----------------------------------------------------------------------

    async def run_post_trade_checks(self, trade: Trade) -> list[SurveillanceEvent]:
        """Run wash-trading and front-running checks after a trade is created."""
        events: list[SurveillanceEvent] = []

        wash_event = await self.check_wash_trading(trade)
        if wash_event:
            events.append(wash_event)

        fr_event = await self.check_front_running(trade)
        if fr_event:
            events.append(fr_event)

        return events

    async def run_post_cancel_checks(
        self, cancelled_order: OrderBookOrder
    ) -> list[SurveillanceEvent]:
        """Run spoofing and layering checks after an order is cancelled."""
        events: list[SurveillanceEvent] = []

        spoof_event = await self.check_spoofing(cancelled_order)
        if spoof_event:
            events.append(spoof_event)

        layer_event = await self.check_layering(cancelled_order)
        if layer_event:
            events.append(layer_event)

        return events
