"""Focused staging-contract tests for the compact buyer-map response."""

from datetime import UTC, datetime
from decimal import Decimal
from unittest.mock import AsyncMock

import pytest

from app.market_catalog import (
    DELIVERY_POINTS_BY_ID,
    DELIVERY_POINTS_BY_NAME,
    PRODUCTS_BY_ID,
    PRODUCT_IDS,
)
from app.routers import orderbook
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.schemas.orderbook import AggregatedOrderbookResponse, OrderSide


UCOME = PRODUCTS_BY_ID[PRODUCT_IDS["UCOME_B100"]]
BIO_METHANOL = PRODUCTS_BY_ID[PRODUCT_IDS["BIO_METHANOL"]]
E_METHANOL = PRODUCTS_BY_ID[PRODUCT_IDS["E_METHANOL"]]
assert UCOME.available_delivery_point_ids is not None
SINGAPORE = DELIVERY_POINTS_BY_ID[UCOME.available_delivery_point_ids[0]]
ROTTERDAM = DELIVERY_POINTS_BY_NAME["Rotterdam"]
OBSERVED_AT = datetime(2026, 10, 2, tzinfo=UTC)


def _group(
    *,
    product,
    delivery_point,
    side: OrderSide,
    window: str,
    minimum: str,
    maximum: str,
    quantity: str,
    order_count: int,
    evidence_class: str = "REAL",
    observed_at: datetime = OBSERVED_AT,
) -> AggregatedOrderbookResponse:
    is_real = evidence_class == "REAL"
    return AggregatedOrderbookResponse(
        product_id=product.id,
        product_name=product.name,
        market_product=product.market_product.value,
        fuel_type=product.fuel_type,
        delivery_point_id=delivery_point.id,
        delivery_point_name=delivery_point.name,
        availability_window=window,
        region=delivery_point.region,
        side=side,
        min_price=Decimal(minimum),
        max_price=Decimal(maximum),
        total_quantity=Decimal(quantity),
        order_count=order_count,
        product_total_order_count=99,
        evidence_class=evidence_class,
        source_kind=(
            MarketSourceKind.LIVE_ORDER if is_real else MarketSourceKind.DEMO_SEED
        ),
        scope=MarketScope.DELIVERY_POINT,
        demo_status=(
            MarketDemoStatus.REAL_ONLY if is_real else MarketDemoStatus.DEMO_ONLY
        ),
        observed_at=observed_at,
    )


def test_compact_groups_preserve_all_window_values_and_native_demo_groups():
    later = datetime(2026, 10, 3, tzinfo=UTC)
    groups = [
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.BID,
            window="SPOT",
            minimum="900",
            maximum="950",
            quantity="10",
            order_count=2,
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.ASK,
            window="SPOT",
            minimum="1000",
            maximum="1050",
            quantity="8",
            order_count=1,
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.BID,
            window="2026-11",
            minimum="880",
            maximum="930",
            quantity="5",
            order_count=3,
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.ASK,
            window="2027-Q1",
            minimum="1020",
            maximum="1100",
            quantity="7",
            order_count=4,
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.BID,
            window="2028-CAL",
            minimum="800",
            maximum="920",
            quantity="2",
            order_count=1,
            observed_at=later,
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.BID,
            window="SPOT",
            minimum="940",
            maximum="940",
            quantity="4",
            order_count=1,
            evidence_class="DEMO",
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.ASK,
            window="SPOT",
            minimum="1000",
            maximum="1000",
            quantity="6",
            order_count=1,
            evidence_class="DEMO",
        ),
        _group(
            product=BIO_METHANOL,
            delivery_point=SINGAPORE,
            side=OrderSide.ASK,
            window="2026-11",
            minimum="1010",
            maximum="1030",
            quantity="9",
            order_count=2,
            evidence_class="DEMO",
        ),
    ]

    markets, demo_groups = orderbook._compact_map_groups(groups)

    real = next(market for market in markets if market.evidence_class == "REAL")
    demo = next(market for market in markets if market.evidence_class == "DEMO")
    assert real.market_product == "BIO_METHANOL"
    assert real.bid_min_price == Decimal("800")
    assert real.bid_max_price == Decimal("950")
    assert real.bid_total_quantity == Decimal("17")
    assert real.bid_order_count == 6
    assert real.ask_min_price == Decimal("1000")
    assert real.ask_max_price == Decimal("1100")
    assert real.ask_total_quantity == Decimal("15")
    assert real.ask_order_count == 5
    assert real.spot_best_bid == Decimal("950")
    assert real.spot_best_ask == Decimal("1000")
    assert real.observed_at == later
    assert real.source_kind == MarketSourceKind.LIVE_ORDER
    assert real.demo_status == MarketDemoStatus.REAL_ONLY
    assert demo.ask_total_quantity == Decimal("15")
    assert demo.ask_order_count == 3
    assert demo.spot_best_bid == Decimal("940")
    assert demo.spot_best_ask == Decimal("1000")
    assert demo.source_kind == MarketSourceKind.DEMO_SEED
    assert demo.demo_status == MarketDemoStatus.DEMO_ONLY
    assert demo_groups == [groups[6], groups[5]]


def test_compact_groups_preserve_ucome_supported_lane_without_inventing_fallback():
    ucome_ask = _group(
        product=UCOME,
        delivery_point=SINGAPORE,
        side=OrderSide.ASK,
        window="SPOT",
        minimum="1250",
        maximum="1260",
        quantity="50",
        order_count=2,
    )

    markets, demo_groups = orderbook._compact_map_groups([ucome_ask])

    assert UCOME.available_delivery_point_ids == (SINGAPORE.id,)
    assert len(markets) == 1
    assert markets[0].product_id == PRODUCTS_BY_ID[UCOME.id].id
    assert markets[0].market_product == "UCOME_B100"
    assert markets[0].fuel_type == "FAME"
    assert markets[0].delivery_point_id == SINGAPORE.id
    assert markets[0].spot_best_bid is None
    assert markets[0].spot_best_ask == Decimal("1250")
    assert demo_groups == []


def test_compact_groups_select_nearest_forward_demo_window_without_spot():
    def demo(side, window, minimum, maximum):
        return _group(
            product=E_METHANOL,
            delivery_point=ROTTERDAM,
            side=side,
            window=window,
            minimum=minimum,
            maximum=maximum,
            quantity="3",
            order_count=1,
            evidence_class="DEMO",
        )

    month_ask = demo(OrderSide.ASK, "2027-01", "700", "710")
    month_bid = demo(OrderSide.BID, "2027-01", "680", "690")
    quarter_ask = demo(OrderSide.ASK, "2027-Q1", "720", "730")
    calendar_ask = demo(OrderSide.ASK, "2027-CAL", "740", "750")

    _, demo_groups = orderbook._compact_map_groups(
        [quarter_ask, calendar_ask, month_bid, month_ask]
    )

    assert demo_groups == [month_ask, month_bid]
    assert {group.side for group in demo_groups} == {OrderSide.BID, OrderSide.ASK}


@pytest.mark.asyncio
async def test_compact_endpoint_reuses_both_public_queries_once(monkeypatch):
    aggregate = AsyncMock(return_value=[])
    recent = AsyncMock(return_value=[])
    monkeypatch.setattr(orderbook, "_aggregate_orderbook", aggregate)
    monkeypatch.setattr(orderbook, "_latest_public_asks_by_delivery_point", recent)
    db = object()

    response = await orderbook.get_compact_map_summary(db=db)

    assert response.markets == []
    assert response.demo_groups == []
    assert response.recent_asks == []
    aggregate.assert_awaited_once_with(db)
    recent.assert_awaited_once_with(db)
