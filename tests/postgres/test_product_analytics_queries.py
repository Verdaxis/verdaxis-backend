"""PostgreSQL correctness for the authoritative Product Analytics aggregates.

Re-runs the frozen fixture contract against a migrated PostgreSQL 17/PostGIS
3.6 database (matching the deployed image) and adds the PostgreSQL-only
concerns the SQLite harness cannot prove: session-timezone independence of
UTC buckets, native UUID/enum round-trips, and the exact commission
payment-date projection.
"""

from __future__ import annotations

from datetime import UTC, datetime
from decimal import Decimal
from uuid import uuid4

from sqlalchemy import event

from app.models.orderbook import OrderBookOrder, OrderBookStatus, OrderSide
from app.schemas.product_analytics import AnalyticsActivity, ProductAnalyticsQuery
from app.services.product_analytics import ProductAnalyticsService
from tests.unit.fixtures import product_analytics as fixtures

EXPECTED = fixtures.EXPECTED


def _query(**overrides) -> ProductAnalyticsQuery:
    values = {"start": fixtures.PERIOD_START, "end": fixtures.PERIOD_END}
    values.update(overrides)
    return ProductAnalyticsQuery(**values)


async def test_full_aggregate_parity_with_frozen_contract(pg_session):
    _engine, session = pg_session
    await fixtures.seed_product_analytics_scenario(session)
    service = ProductAnalyticsService(session)

    overview = await service.overview(_query())
    orders = EXPECTED["orders"]
    trades = EXPECTED["trades"]
    assert overview.kpis.participating_organizations.value == orders["participating_organizations_current"]
    assert overview.kpis.live_orders.value == orders["live_current"]
    assert overview.kpis.live_orders.previous == orders["live_previous"]
    assert overview.kpis.confirmed_trades.value == trades["confirmed_current_with_legacy_fallback"]
    assert overview.lifecycle.registered.value == EXPECTED["members"]["registered_current"]
    assert overview.lifecycle.trading.value == trades["trading_organizations_current"]
    assert overview.lifecycle.retained.value == EXPECTED["retention"]["retained_organizations"]
    assert overview.data_quality.legacy_timestamp_fallback_count == trades["legacy_timestamp_fallback_count"]

    marketplace = await service.marketplace(_query(activity=AnalyticsActivity.ALL))
    live = marketplace.live
    assert live.kpis.execution_rate.numerator == orders["execution_rate_numerator"]
    assert live.kpis.execution_rate.denominator == orders["execution_rate_denominator"]
    assert live.kpis.confirmed_volume_mt.value == Decimal(
        trades["confirmed_volume_current_with_legacy_fallback_mt"]
    )
    assert marketplace.commercial.realized_gmv_usd.value == Decimal(
        trades["realized_gmv_current_usd"]
    )
    assert marketplace.commercial.realized_revenue_usd.value == Decimal(
        trades["realized_revenue_current_usd"]
    )
    assert marketplace.demo.kpis.confirmed_trades.value == trades["demo_trades_current"]
    assert marketplace.unknown.kpis.open_bids.value == orders["unknown_current"]

    slice_rows = {
        (row.product_key, row.delivery_point_key): row for row in live.liquidity.slices
    }
    bm_sg = slice_rows[("BIO_METHANOL", "singapore")]
    frozen = EXPECTED["liquidity"]["bm_sg_spot"]
    assert bm_sg.best_bid_usd_per_mt == Decimal(frozen["best_bid_usd_per_mt"])
    assert bm_sg.one_percent_bid_depth_mt == Decimal(frozen["one_percent_bid_depth_mt"])
    assert live.liquidity.median_open_order_age_hours == Decimal(
        str(EXPECTED["liquidity"]["median_open_order_age_hours"])
    )

    retention = await service.retention(_query())
    assert retention.retained_organizations.value == EXPECTED["retention"]["retained_organizations"]
    assert retention.reactivated_organizations.value == EXPECTED["retention"]["reactivated_organizations"]

    activation = await service.activation(_query())
    assert activation.registered_total.value == EXPECTED["members"]["registered_current"]
    drop_off = {cell.key: cell for cell in activation.drop_off}
    assert drop_off["unverified"].count == EXPECTED["activation"]["drop_off_unverified"]
    assert drop_off["never_logged_in"].suppressed is True


async def test_utc_buckets_are_independent_of_session_timezone(pg_session):
    """§1.4 rule 7: buckets are UTC calendar dates even when the PostgreSQL
    session timezone is not UTC."""
    _engine, session = pg_session
    await fixtures.seed_product_analytics_scenario(session)

    # A late-evening UTC order would fall on the next calendar day in
    # Singapore; a timezone-leaky bucket would misplace it.
    session.add(
        OrderBookOrder(
            id=uuid4(),
            organization_id=fixtures.LIVE_BUYER_ORG_ID,
            side=OrderSide.BID,
            product_id=fixtures.PRODUCT_BIO_METHANOL_ID,
            delivery_point_id=fixtures.DELIVERY_POINT_SINGAPORE_ID,
            quantity_mt=Decimal("10"),
            remaining_quantity_mt=Decimal("10"),
            price_per_mt_usd=Decimal("781"),
            availability_window="SPOT",
            status=OrderBookStatus.OPEN,
            created_at=datetime(2026, 6, 30, 23, 30, tzinfo=UTC),
        )
    )
    await session.commit()

    from sqlalchemy import text

    await session.execute(text("SET TIME ZONE 'Asia/Singapore'"))
    service = ProductAnalyticsService(session)
    result = await service.overview(_query())

    by_date = {point.date.isoformat(): point.value for point in result.orders_series}
    assert by_date["2026-06-30"] == 1  # 23:30 UTC stays on the UTC date
    assert by_date["2026-06-07"] == 1
    assert sum(point.value for point in result.orders_series) == EXPECTED["orders"]["live_current"] + 1


async def test_commission_payment_date_midday_projection_on_postgres(pg_session):
    _engine, session = pg_session
    await fixtures.seed_product_analytics_scenario(session)
    service = ProductAnalyticsService(session)

    midday_start = await service.marketplace(
        _query(
            start=datetime(2026, 6, 25, 12, tzinfo=UTC),
            end=datetime(2026, 6, 30, 12, tzinfo=UTC),
            compare=False,
            activity=AnalyticsActivity.LIVE,
        )
    )
    assert midday_start.commercial.realized_revenue_usd.value == Decimal("0.00")

    midday_end = await service.marketplace(
        _query(
            start=datetime(2026, 6, 20, tzinfo=UTC),
            end=datetime(2026, 6, 25, 12, tzinfo=UTC),
            compare=False,
            activity=AnalyticsActivity.LIVE,
        )
    )
    assert midday_end.commercial.realized_revenue_usd.value == Decimal(
        EXPECTED["trades"]["realized_revenue_current_usd"]
    )


async def test_statement_budgets_hold_on_postgres(pg_session):
    engine, session = pg_session
    await fixtures.seed_product_analytics_scenario(session)
    service = ProductAnalyticsService(session)

    counter = {"statements": 0}

    def _on_execute(_conn, _cursor, _statement, _parameters, _context, _executemany):
        counter["statements"] += 1

    event.listen(engine.sync_engine, "before_cursor_execute", _on_execute)
    try:
        counter["statements"] = 0
        await service.overview(_query())
        assert counter["statements"] <= 8

        counter["statements"] = 0
        await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
        assert counter["statements"] <= 8

        counter["statements"] = 0
        await service.marketplace(_query(activity=AnalyticsActivity.ALL))
        assert counter["statements"] <= 10
    finally:
        event.remove(engine.sync_engine, "before_cursor_execute", _on_execute)


async def test_native_uuid_and_enum_round_trip(pg_session):
    _engine, session = pg_session
    scenario = await fixtures.seed_product_analytics_scenario(session)
    service = ProductAnalyticsService(session)

    assert scenario.trades[0].id == fixtures.TRADE_IDS["paid"]
    marketplace = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    statuses = {cell.key for cell in marketplace.live.order_status_distribution}
    # Enum values decode to their canonical string names through the
    # cross-source UNION, on native PostgreSQL types.
    assert statuses <= {"OPEN", "PARTIALLY_FILLED", "FILLED", "CANCELLED", "EXPIRED"}
    reference = await service.marketplace(_query(activity=AnalyticsActivity.REFERENCE))
    assert all(
        row.product_key in {"BIO_METHANOL", "E_METHANOL"}
        for row in reference.reference.rows
    )
