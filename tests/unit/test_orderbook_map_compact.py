"""Focused tests for the compact buyer-map response."""

from datetime import UTC, datetime
from decimal import Decimal
from unittest.mock import AsyncMock
from uuid import UUID

import pytest

from app.routers import orderbook
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.schemas.orderbook import AggregatedOrderbookResponse, OrderSide


PRODUCT_ID = UUID("10000000-0000-0000-0000-000000000001")
OTHER_PRODUCT_ID = UUID("10000000-0000-0000-0000-000000000002")
DELIVERY_POINT_ID = UUID("20000000-0000-0000-0000-000000000001")
OTHER_DELIVERY_POINT_ID = UUID("20000000-0000-0000-0000-000000000002")
OBSERVED_AT = datetime(2026, 10, 2, tzinfo=UTC)


def _group(
    *,
    side: OrderSide,
    window: str,
    minimum: str,
    maximum: str,
    quantity: str,
    order_count: int,
    evidence_class: str = "REAL",
    product_id: UUID = PRODUCT_ID,
    delivery_point_id: UUID = DELIVERY_POINT_ID,
    market_product: str = "UCOME_B100",
    product_name: str = "UCOME B100",
    delivery_point_name: str = "Singapore",
    observed_at: datetime = OBSERVED_AT,
) -> AggregatedOrderbookResponse:
    is_real = evidence_class == "REAL"
    return AggregatedOrderbookResponse(
        product_id=product_id,
        product_name=product_name,
        market_product=market_product,
        fuel_type="Biofuel",
        delivery_point_id=delivery_point_id,
        delivery_point_name=delivery_point_name,
        availability_window=window,
        region="Asia",
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


def test_compact_groups_preserve_all_window_totals_extrema_and_spot_inputs():
    later = datetime(2026, 10, 3, tzinfo=UTC)
    groups = [
        _group(
            side=OrderSide.BID,
            window="SPOT",
            minimum="900",
            maximum="950",
            quantity="10",
            order_count=2,
        ),
        _group(
            side=OrderSide.ASK,
            window="SPOT",
            minimum="1000",
            maximum="1050",
            quantity="8",
            order_count=1,
        ),
        _group(
            side=OrderSide.BID,
            window="2026-11",
            minimum="880",
            maximum="930",
            quantity="5",
            order_count=3,
        ),
        _group(
            side=OrderSide.ASK,
            window="2027-Q1",
            minimum="1020",
            maximum="1100",
            quantity="7",
            order_count=4,
        ),
        _group(
            side=OrderSide.BID,
            window="2028-CAL",
            minimum="800",
            maximum="920",
            quantity="2",
            order_count=1,
            observed_at=later,
        ),
        _group(
            side=OrderSide.BID,
            window="SPOT",
            minimum="940",
            maximum="940",
            quantity="4",
            order_count=1,
            evidence_class="DEMO",
        ),
        _group(
            side=OrderSide.ASK,
            window="SPOT",
            minimum="1000",
            maximum="1000",
            quantity="6",
            order_count=1,
            evidence_class="DEMO",
        ),
        _group(
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

    assert len(markets) == 2
    real = next(market for market in markets if market.evidence_class == "REAL")
    demo = next(market for market in markets if market.evidence_class == "DEMO")

    assert real.market_product == "UCOME_B100"
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
    assert all(group.availability_window == "SPOT" for group in demo_groups)


def test_compact_groups_keep_one_sided_values_and_nearest_forward_demo_window():
    month_ask = _group(
        side=OrderSide.ASK,
        window="2027-01",
        minimum="700",
        maximum="710",
        quantity="3",
        order_count=2,
        evidence_class="DEMO",
        product_id=OTHER_PRODUCT_ID,
        delivery_point_id=OTHER_DELIVERY_POINT_ID,
        market_product="NEW_CATALOG_PRODUCT",
        product_name="New Catalog Product",
        delivery_point_name="Rotterdam",
    )
    month_bid = _group(
        side=OrderSide.BID,
        window="2027-01",
        minimum="680",
        maximum="690",
        quantity="5",
        order_count=1,
        evidence_class="DEMO",
        product_id=OTHER_PRODUCT_ID,
        delivery_point_id=OTHER_DELIVERY_POINT_ID,
        market_product="NEW_CATALOG_PRODUCT",
        product_name="New Catalog Product",
        delivery_point_name="Rotterdam",
    )
    quarter_ask = _group(
        side=OrderSide.ASK,
        window="2027-Q1",
        minimum="720",
        maximum="730",
        quantity="4",
        order_count=1,
        evidence_class="DEMO",
        product_id=OTHER_PRODUCT_ID,
        delivery_point_id=OTHER_DELIVERY_POINT_ID,
        market_product="NEW_CATALOG_PRODUCT",
        product_name="New Catalog Product",
        delivery_point_name="Rotterdam",
    )
    calendar_ask = _group(
        side=OrderSide.ASK,
        window="2027-CAL",
        minimum="740",
        maximum="750",
        quantity="2",
        order_count=1,
        evidence_class="DEMO",
        product_id=OTHER_PRODUCT_ID,
        delivery_point_id=OTHER_DELIVERY_POINT_ID,
        market_product="NEW_CATALOG_PRODUCT",
        product_name="New Catalog Product",
        delivery_point_name="Rotterdam",
    )
    one_sided_real = _group(
        side=OrderSide.ASK,
        window="SPOT",
        minimum="800",
        maximum="810",
        quantity="11",
        order_count=2,
    )

    markets, demo_groups = orderbook._compact_map_groups(
        [quarter_ask, calendar_ask, month_bid, one_sided_real, month_ask]
    )

    real = next(market for market in markets if market.evidence_class == "REAL")
    assert real.bid_min_price is None
    assert real.bid_max_price is None
    assert real.bid_total_quantity == Decimal("0")
    assert real.bid_order_count == 0
    assert real.spot_best_bid is None
    assert real.spot_best_ask == Decimal("800")

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
