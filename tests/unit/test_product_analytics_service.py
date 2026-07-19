"""Authoritative Product Analytics service tests against the frozen fixture
contract (tests/unit/fixtures/product_analytics.py).

Every assertion traces to a hand-derived value in ``fixtures.EXPECTED`` or to
a plan rule (fe/docs/plans/2026-07-15-product-analytics-workspace.md). The
SQLite harness here is the fast unit check; tests/postgres re-runs the same
aggregates against a migrated PostgreSQL 17/PostGIS 3.6 database.
"""

from __future__ import annotations

from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from decimal import Decimal

import pytest
from sqlalchemy import event
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.schemas.product_analytics import (
    AnalyticsActivity,
    ProductAnalyticsQuery,
)
from app.services.product_analytics import ProductAnalyticsService
from tests.unit.fixtures import product_analytics as fixtures

EXPECTED = fixtures.EXPECTED


@pytest.fixture
async def seeded_engine():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=list(fixtures.scenario_tables()))
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        await fixtures.seed_product_analytics_scenario(session)
        await fixtures.seed_fact_tables(session)
        yield engine, session
    await engine.dispose()


@pytest.fixture
async def pre_fact_engine():
    """Scenario without any fact rows: coverage-gated metrics must be null."""
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all, tables=list(fixtures.scenario_tables()))
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        await fixtures.seed_product_analytics_scenario(session)
        yield engine, session
    await engine.dispose()


def _query(**overrides) -> ProductAnalyticsQuery:
    values = {"start": fixtures.PERIOD_START, "end": fixtures.PERIOD_END}
    values.update(overrides)
    return ProductAnalyticsQuery(**values)


@asynccontextmanager
async def count_statements(engine):
    counter = {"statements": 0}

    def _on_execute(_conn, _cursor, _statement, _parameters, _context, _executemany):
        counter["statements"] += 1

    event.listen(engine.sync_engine, "before_cursor_execute", _on_execute)
    try:
        yield counter
    finally:
        event.remove(engine.sync_engine, "before_cursor_execute", _on_execute)


# ---------------------------------------------------------------------------
# Overview
# ---------------------------------------------------------------------------


async def test_overview_matches_frozen_expectations(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.overview(_query())

    members = EXPECTED["members"]
    orders = EXPECTED["orders"]
    trades = EXPECTED["trades"]
    retention = EXPECTED["retention"]

    kpis = result.kpis
    # Fact-backed KPIs: qualified organizations reconstructed as-of end from
    # the transition history; active members from login-day facts. The
    # previous active-member window predates login coverage → null.
    assert kpis.qualified_organizations.value == members["qualified_organizations_as_of_end"]
    assert (
        kpis.qualified_organizations.previous
        == members["qualified_organizations_as_of_previous_end"]
    )
    assert kpis.active_members.value == members["active_current"]
    assert kpis.active_members.previous is None
    assert kpis.participating_organizations.value == orders["participating_organizations_current"]
    assert kpis.participating_organizations.previous == orders["participating_organizations_previous"]
    assert kpis.live_orders.value == orders["live_current"]
    assert kpis.live_orders.previous == orders["live_previous"]
    # Confirmed trades include the documented legacy created_at fallback and
    # flag it in data quality.
    assert kpis.confirmed_trades.value == trades["confirmed_current_with_legacy_fallback"]
    assert kpis.confirmed_trades.previous == trades["confirmed_previous_strict"]
    assert result.data_quality.legacy_timestamp_fallback_count == trades["legacy_timestamp_fallback_count"]
    assert result.data_quality.missing_paid_at_count == trades["missing_paid_at_count"]

    lifecycle = result.lifecycle
    assert lifecycle.registered.value == members["registered_current"]
    assert lifecycle.registered.previous == members["registered_previous"]
    assert lifecycle.trading.value == trades["trading_organizations_current"]
    assert lifecycle.trading.previous == trades["trading_organizations_previous"]
    assert lifecycle.retained.value == retention["retained_organizations"]
    assert lifecycle.retained.previous == retention["retained_organizations_previous"]

    assert result.dormant_approved_members == members["dormant_approved"]
    assert result.one_sided_live_market is False


async def test_overview_series_are_dense_utc_daily_buckets(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.overview(_query())

    assert len(result.orders_series) == 30
    assert sum(point.value for point in result.orders_series) == EXPECTED["orders"]["live_current"]
    by_date = {point.date.isoformat(): point.value for point in result.orders_series}
    assert by_date["2026-06-07"] == 1
    assert by_date["2026-06-02"] == 0  # dense zeros, not missing days

    assert len(result.confirmed_trades_series) == 30
    assert (
        sum(point.value for point in result.confirmed_trades_series)
        == EXPECTED["trades"]["confirmed_current_with_legacy_fallback"]
    )


async def test_overview_marketplace_balance_applies_small_cell_suppression(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.overview(_query())
    balance = result.marketplace_balance

    # Two bid-side organizations and one ask-side organization are both under
    # the three-entity threshold: suppressed, not zeroed (§1.4 rule 12).
    assert balance.buyer_organizations.suppressed is True
    assert balance.buyer_organizations.value is None
    assert balance.supplier_organizations.suppressed is True
    # Headline order counts stay visible.
    assert balance.bid_orders.value == EXPECTED["orders"]["live_bids_current"]
    assert balance.ask_orders.value == EXPECTED["orders"]["live_asks_current"]


async def test_overview_uses_a_bounded_number_of_statements(seeded_engine):
    engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    async with count_statements(engine) as counter:
        await service.overview(_query())
    assert counter["statements"] <= 8, f"overview used {counter['statements']} statements"


# ---------------------------------------------------------------------------
# Marketplace — live section
# ---------------------------------------------------------------------------


async def test_marketplace_live_kpis_and_execution(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    live = result.live

    orders = EXPECTED["orders"]
    trades = EXPECTED["trades"]
    assert result.demo is None and result.unknown is None and result.reference is None
    assert live.kpis.participating_organizations.value == orders["participating_organizations_current"]
    assert live.kpis.open_bids.value == orders["live_bids_current"]
    assert live.kpis.open_asks.value == orders["live_asks_current"]
    assert live.kpis.confirmed_trades.value == trades["confirmed_current_with_legacy_fallback"]
    assert live.kpis.confirmed_volume_mt.value == Decimal(
        trades["confirmed_volume_current_with_legacy_fallback_mt"]
    )
    assert live.kpis.confirmed_volume_mt.previous == Decimal(trades["confirmed_volume_previous_mt"])

    execution = live.kpis.execution_rate
    assert execution.numerator == orders["execution_rate_numerator"]
    assert execution.denominator == orders["execution_rate_denominator"]
    assert execution.rate_pct == Decimal("42.86")
    # The period closed in the past: later fills can never improve it.
    assert execution.cohort_complete is True


async def test_marketplace_live_slice_liquidity(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    liquidity = result.live.liquidity
    expected = EXPECTED["liquidity"]

    assert liquidity.two_sided_slices == expected["two_sided_slices"]
    assert liquidity.one_sided_slices == expected["one_sided_slices"]
    assert liquidity.crossed_slices == expected["crossed_slices"]
    assert liquidity.median_open_order_age_hours == Decimal(
        str(expected["median_open_order_age_hours"])
    )
    assert liquidity.median_hours_to_first_fill == Decimal(
        str(expected["median_hours_to_first_fill"])
    )

    by_slice = {
        (row.product_key, row.delivery_point_key, row.availability_window): row
        for row in liquidity.slices
    }
    bm_sg = by_slice[("BIO_METHANOL", "singapore", "SPOT")]
    frozen = expected["bm_sg_spot"]
    assert bm_sg.suppressed is False
    assert bm_sg.contributing_organizations == frozen["distinct_live_organizations"]
    assert bm_sg.best_bid_usd_per_mt == Decimal(frozen["best_bid_usd_per_mt"])
    assert bm_sg.best_ask_usd_per_mt == Decimal(frozen["best_ask_usd_per_mt"])
    assert bm_sg.spread_usd_per_mt == Decimal(frozen["spread_usd_per_mt"])
    assert bm_sg.spread_bps == Decimal("253.16")  # 20 / 790 × 10,000
    assert bm_sg.best_bid_depth_mt == Decimal(frozen["best_bid_depth_mt"])
    assert bm_sg.best_ask_depth_mt == Decimal(frozen["best_ask_depth_mt"])
    assert bm_sg.one_percent_bid_depth_mt == Decimal(frozen["one_percent_bid_depth_mt"])
    assert bm_sg.one_percent_ask_depth_mt == Decimal(frozen["one_percent_ask_depth_mt"])
    assert bm_sg.crossed is False

    # One contributing organization: price/depth/spread suppressed while the
    # slice still counts as one-sided.
    bm_rt = by_slice[("BIO_METHANOL", "rotterdam", "SPOT")]
    assert bm_rt.suppressed is True
    assert bm_rt.best_ask_usd_per_mt is None
    assert bm_rt.best_bid_depth_mt is None


async def test_marketplace_live_matrix_windows_and_statuses(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    live = result.live

    matrix = {
        (cell.product_key, cell.delivery_point_key): cell for cell in live.product_port_matrix
    }
    bm_sg = matrix[("BIO_METHANOL", "singapore")]
    assert bm_sg.orders.count == 6  # 4 bids + 2 asks created in the period
    assert bm_sg.organizations.count == 3
    bm_rt = matrix[("BIO_METHANOL", "rotterdam")]
    assert bm_rt.orders.suppressed is True  # single contributing organization
    assert bm_rt.organizations.suppressed is True

    windows = {row.key: row for row in live.window_distribution}
    assert windows["SPOT"].count == EXPECTED["orders"]["live_current"]
    assert windows["SPOT"].share_pct == Decimal("100.00")

    order_statuses = {cell.key: cell for cell in live.order_status_distribution}
    assert order_statuses["OPEN"].count == 3  # three organizations → visible
    assert order_statuses["FILLED"].suppressed is True  # two organizations
    trade_statuses = {cell.key: cell for cell in live.trade_status_distribution}
    # Trade cells fall back to the event-count rule; every live status has
    # fewer than three trades in the period.
    assert all(cell.suppressed for cell in trade_statuses.values())
    assert set(trade_statuses) == {
        "PAID",
        "CONFIRMED",
        "PENDING_CONFIRMATION",
        "CANCELLED",
        "DECLINED",
    }


async def test_marketplace_commercial_summary_and_commission_semantics(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    commercial = result.commercial
    trades = EXPECTED["trades"]

    # GMV: final_total_usd on PAID trades bucketed by paid_at. The PAID trade
    # with no paid_at is excluded and surfaces in data quality instead.
    assert commercial.realized_gmv_usd.value == Decimal(trades["realized_gmv_current_usd"])
    assert result.data_quality.missing_paid_at_count == trades["missing_paid_at_count"]

    # Revenue: Commission.amount_usd, status PAID, bucketed by payment_date.
    # A paid trade with no paid commission row contributes zero; a PAID
    # commission with a null payment_date is excluded and counted.
    assert commercial.realized_revenue_usd.value == Decimal(trades["realized_revenue_current_usd"])
    assert (
        result.data_quality.missing_commission_payment_date_count
        == trades["missing_commission_payment_date_count"]
    )
    assert commercial.commission_pending_usd == Decimal(
        trades["commission_outstanding_pending_usd"]
    )
    assert commercial.commission_invoiced_usd == Decimal(
        trades["commission_outstanding_invoiced_usd"]
    )


async def test_commission_payment_date_midday_boundary_projection(seeded_engine):
    """payment_date projects to 00:00:00Z: a midday start excludes that date,
    a midday end includes it (§1.5)."""
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    # Payment date 2026-06-25. Midday start on 6/25 → projected instant
    # (6/25 00:00Z) < start → excluded.
    midday_start = await service.marketplace(
        _query(
            start=datetime(2026, 6, 25, 12, tzinfo=UTC),
            end=datetime(2026, 6, 30, 12, tzinfo=UTC),
            compare=False,
            activity=AnalyticsActivity.LIVE,
        )
    )
    assert midday_start.commercial.realized_revenue_usd.value == Decimal("0.00")

    # Midday end on 6/25 → projected instant < end → included.
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


async def test_execution_cohort_is_right_censored_for_open_periods(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    now = datetime.now(UTC)
    result = await service.marketplace(
        _query(
            start=now - timedelta(days=1),
            end=now + timedelta(days=1),
            compare=False,
            activity=AnalyticsActivity.LIVE,
        )
    )
    # No orders were created inside this narrow window, so the cohort is
    # trivially complete even though the period is still open.
    assert result.live.kpis.execution_rate.denominator == 0
    assert result.live.kpis.execution_rate.cohort_complete is True

    open_ended = await service.marketplace(
        _query(
            start=fixtures.PERIOD_START,
            end=now + timedelta(days=1),
            compare=False,
            activity=AnalyticsActivity.LIVE,
        )
    )
    # Open, unexpired orders exist in the window and the period has not
    # closed: the execution cohort can still change.
    assert open_ended.live.kpis.execution_rate.cohort_complete is False
    assert open_ended.data_quality.cohort_complete is False


# ---------------------------------------------------------------------------
# Marketplace — demo/unknown/reference separation
# ---------------------------------------------------------------------------


async def test_marketplace_all_returns_separate_unsummed_sections(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.ALL))

    live, demo, unknown = result.live, result.demo, result.unknown
    assert live is not None and demo is not None and unknown is not None

    # Demo: one open bid, one demo-contaminated (mixed-party) trade. Mixed
    # trades are demo, never live (§1.4 rule 14).
    assert demo.kpis.open_bids.value == EXPECTED["orders"]["demo_current"]
    assert demo.kpis.confirmed_trades.value == EXPECTED["trades"]["demo_trades_current"]
    # Unknown: the pending-only organization's order.
    assert unknown.kpis.open_bids.value == EXPECTED["orders"]["unknown_current"]
    assert unknown.kpis.confirmed_trades.value == EXPECTED["trades"]["unknown_trades_current"]

    # Live figures are identical to a LIVE-only request: nothing was added.
    live_only = await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    assert live.kpis == live_only.live.kpis

    # Demo liquidity stays in the demo section (single demo organization →
    # suppressed slice) and never inflates live depth.
    assert demo.liquidity.one_sided_slices == 1
    assert all(row.suppressed for row in demo.liquidity.slices)
    live_sg = next(
        row for row in live.liquidity.slices if row.delivery_point_key == "singapore"
    )
    assert live_sg.one_percent_bid_depth_mt == Decimal(
        EXPECTED["liquidity"]["bm_sg_spot"]["one_percent_bid_depth_mt"]
    )


async def test_marketplace_reference_is_coverage_not_activity(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.REFERENCE))

    assert result.live is None and result.demo is None and result.unknown is None
    assert result.commercial is None
    rows = result.reference.rows
    # Two canonical products × two active delivery points at SPOT.
    assert len(rows) == 4
    for row in rows:
        # Seeded benchmark quotes carry a price but no observation timestamp:
        # stale, window-adjusted, seeded provenance.
        assert row.coverage_status == "stale"
        assert row.source_label.value == "SEED_MATRIX"
        assert row.source_kind.value == "SEEDED_BENCHMARK"
        assert row.benchmark_price_usd_per_mt is not None
        assert row.observed_at is None
    by_key = {(row.product_key, row.delivery_point_key): row for row in rows}
    # Seed matrix midpoint for Bio Methanol / Singapore: (1020+1070+1090+1140)/4.
    assert by_key[("BIO_METHANOL", "singapore")].benchmark_price_usd_per_mt == Decimal("1080.00")


async def test_marketplace_statement_budgets(seeded_engine):
    engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    async with count_statements(engine) as counter:
        await service.marketplace(_query(activity=AnalyticsActivity.LIVE))
    assert counter["statements"] <= 8, f"live used {counter['statements']}"

    async with count_statements(engine) as counter:
        await service.marketplace(_query(activity=AnalyticsActivity.REFERENCE))
    assert counter["statements"] <= 8, f"reference used {counter['statements']}"

    async with count_statements(engine) as counter:
        await service.marketplace(_query(activity=AnalyticsActivity.ALL))
    # ALL composes live/demo/unknown plus the two bounded catalog/benchmark
    # lookups for reference coverage: the documented composite exception.
    assert counter["statements"] <= 10, f"all used {counter['statements']}"


# ---------------------------------------------------------------------------
# Activation and retention
# ---------------------------------------------------------------------------


async def test_activation_matches_frozen_expectations(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.activation(_query())
    expected = EXPECTED["activation"]
    members = EXPECTED["members"]

    assert result.registered_total.value == members["registered_current"]
    assert result.registered_total.previous == members["registered_previous"]
    # Role split is a segmented cell: two buyers → suppressed; zero suppliers
    # stays a visible genuine zero.
    assert result.registered_buyer.suppressed is True
    assert result.registered_supplier.count == 0

    drop_off = {cell.key: cell for cell in result.drop_off}
    assert drop_off["unverified"].count == expected["drop_off_unverified"]
    assert drop_off["organization_incomplete"].count == expected["drop_off_organization_incomplete"]
    for suppressed_key in ("rejected", "pending_approval", "never_logged_in"):
        assert drop_off[suppressed_key].suppressed is True, suppressed_key
        assert drop_off[suppressed_key].count is None

    # One organization reached its first live order inside the period: the
    # sample is below the privacy threshold, so the distribution suppresses.
    assert result.first_live_order_organizations.suppressed is True
    assert result.time_to_first_live_order.suppressed is True
    assert result.time_to_first_live_order.median_hours is None
    assert result.data_quality.suppressed_cell_count > 0

    # Fact-backed journey stages: one approval transition and two first
    # recorded logins in the period — both segmented small cells.
    assert result.approved_members.suppressed is True
    assert result.first_login_members.suppressed is True
    assert result.time_to_first_login.suppressed is True
    assert result.time_to_first_login.median_hours is None
    assert result.status_coverage_start is not None
    assert result.login_coverage_start is not None


async def test_retention_matches_frozen_expectations(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.retention(_query())
    expected = EXPECTED["retention"]

    assert result.retained_organizations.value == expected["retained_organizations"]
    assert result.retained_organizations.previous == expected["retained_organizations_previous"]
    assert result.reactivated_organizations.value == expected["reactivated_organizations"]
    assert (
        result.reactivated_organizations.previous
        == expected["reactivated_organizations_previous"]
    )
    assert result.dormant_approved_members.value == EXPECTED["members"]["dormant_approved"]

    repeat = {cell.key: cell for cell in result.repeat_participation}
    # 1 organization on a single day and 2 on three-plus days are segmented
    # small cells; the empty two-day bucket is a visible zero.
    assert repeat["1"].suppressed is True
    assert repeat["2"].count == expected["repeat_participation_2_days"]
    assert repeat["3_plus"].suppressed is True

    # Returning members require login coverage over both windows; the
    # previous window predates coverage → null, not a fabricated count.
    assert result.returning_members.value is None
    # Weekly member cohorts start at coverage; every pilot-scale cohort cell
    # is suppressed rather than zeroed.
    assert result.member_cohorts, "cohorts expected once login facts exist"
    assert all(row.size.suppressed for row in result.member_cohorts)
    assert result.login_coverage_start == datetime(2026, 5, 20, tzinfo=UTC)


async def test_engagement_rolling_windows_are_coverage_gated(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.engagement(_query())
    expected = EXPECTED["engagement"]

    assert result.kpis.dau.value == expected["dau"]
    assert result.kpis.wau.value == expected["wau"]
    assert result.kpis.mau.value == expected["mau"]
    assert sum(point.value for point in result.active_members_trend) == 4
    assert result.login_coverage_start == datetime(2026, 5, 20, tzinfo=UTC)


async def test_fact_backed_metrics_are_null_without_fact_rows(pre_fact_engine):
    """Before the fact tables have rows, coverage-gated metrics return null —
    never values inferred from User.last_login or User.status snapshots."""
    _engine, session = pre_fact_engine
    service = ProductAnalyticsService(session)

    overview = await service.overview(_query())
    assert overview.kpis.active_members.value is None
    assert overview.kpis.qualified_organizations.value is None
    assert overview.login_coverage_start is None
    assert overview.status_coverage_start is None

    engagement = await service.engagement(_query())
    assert engagement.kpis.mau.value is None

    retention = await service.retention(_query())
    assert retention.returning_members.value is None
    assert retention.member_cohorts == []

    activation = await service.activation(_query())
    assert activation.approved_members.count is None
    assert activation.first_login_members.count == 0


async def test_activation_and_retention_statement_budgets(seeded_engine):
    engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    async with count_statements(engine) as counter:
        await service.activation(_query())
    assert counter["statements"] <= 8

    async with count_statements(engine) as counter:
        await service.retention(_query())
    assert counter["statements"] <= 8


# ---------------------------------------------------------------------------
# Canonical keys and allowlisting
# ---------------------------------------------------------------------------


async def test_no_uuids_or_names_leak_into_breakdown_keys(seeded_engine):
    _engine, session = seeded_engine
    service = ProductAnalyticsService(session)

    result = await service.marketplace(_query(activity=AnalyticsActivity.ALL))

    for section in (result.live, result.demo, result.unknown):
        for row in section.liquidity.slices:
            assert "-" not in row.product_key or not any(
                ch.isdigit() for ch in row.product_key
            ), row.product_key
        for cell in section.order_status_distribution + section.trade_status_distribution:
            assert cell.key.isupper()  # enum values, not free text
    for row in result.reference.rows:
        # Stable catalog keys, never catalog UUIDs.
        assert row.product_key in {"BIO_METHANOL", "E_METHANOL"}
        assert row.delivery_point_key in {"singapore", "rotterdam"}
