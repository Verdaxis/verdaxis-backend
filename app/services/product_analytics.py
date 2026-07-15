"""Authoritative Product Analytics aggregates (PostgreSQL-backed).

Implements the metric definitions frozen in
fe/docs/plans/2026-07-15-product-analytics-workspace.md §1.5 and the fixture
contract in tests/unit/fixtures/product_analytics.py. The service owns:

- The Product Analytics recognized-market predicate: an organization is
  recognized only when it has at least one APPROVED Buyer/Supplier member and
  is not a seeded demo organization. This is deliberately stricter than the
  legacy role-only helper in app/routers/admin_analytics.py, which stays
  untouched.
- Source provenance (§1.4 rule 14): an order is DEMO when its organization is
  seeded demo, LIVE when recognized, UNKNOWN otherwise; a trade is DEMO when
  either party is demo, LIVE only when both parties are recognized, UNKNOWN
  otherwise. Sources are never summed.
- Small-cell suppression (§1.4 rule 12): every segmented cell backed by fewer
  than three underlying users/organizations (or fewer than three events when
  entity counts are unavailable) is returned as a typed suppressed cell.
  Unsegmented headline totals stay visible; genuine zeros stay visible.
- Half-open UTC periods and dense UTC date series (§1.4 rules 7 and 16).

SQL round-trip budget (§2.6): every statement is a grouped aggregate — no
per-user, per-organization, per-product, or per-day loops. Single-source tab
aggregation uses at most eight round trips; a Marketplace ``ALL`` request
composes the demo/unknown/reference sections and costs two additional bounded
catalog/benchmark lookups (ten total), which the runbook documents as the
composite-view exception.

Fields that require the Task-5 fact tables (login history, status
transitions) are returned as ``None`` — never inferred from the mutable
``User.last_login``/``User.status`` snapshots — except where the snapshot is
itself the definition (e.g. "approved members who never logged in").
"""

from __future__ import annotations

import statistics
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal, ROUND_HALF_UP
from typing import Any, Literal
from uuid import UUID

from sqlalchemy import (
    Date,
    Integer,
    String,
    and_,
    case,
    cast,
    distinct,
    exists,
    func,
    literal,
    null,
    or_,
    select,
    union_all,
)
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.catalog import DeliveryPoint, Product, derive_market_product
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
    TradeStatus,
)
from app.models.orders import Commission, CommissionStatus
from app.models.user import User, UserRole, UserStatus
from app.schemas.product_analytics import (
    AggregateCell,
    AnalyticsActivity,
    AnalyticsDataQuality,
    CohortCell,
    CohortRow,
    CommercialSummary,
    ConcentrationBand,
    DecimalMetricValue,
    EngagementKpis,
    DurationDistribution,
    LiquiditySummary,
    MarketActivitySection,
    MarketBalanceTrend,
    MarketplaceBalance,
    MarketplaceKpis,
    MetricValue,
    OrganizationConcentration,
    OverviewKpis,
    ProductAnalyticsQuery,
    ProductPortCell,
    RankedRow,
    RatioValue,
    ReferenceCoverageRow,
    ReferenceCoverageSection,
    ReferenceScope,
    ReferenceSourceKind,
    ReferenceSourceLabel,
    SeriesPoint,
    SliceLiquidityRow,
)
from app.services.availability_windows import availability_window_display_label
from app.services.benchmarks import get_benchmark_quotes
from app.services.demo_market import DEMO_MARKET_ORG_IDS

_MARKET_ROLES = (UserRole.BUYER, UserRole.SUPPLIER)
_CONFIRMED_TRADE_STATUSES = (TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID)
_OPEN_ORDER_STATUSES = (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
_SUPPRESSION_THRESHOLD = 3
_MAX_COHORTS = 54

Provenance = Literal["live", "demo", "unknown"]


def slugify(value: str) -> str:
    """Stable lowercase key for catalog labels (never a UUID)."""
    cleaned = "".join(ch if ch.isalnum() else "-" for ch in value.strip().lower())
    while "--" in cleaned:
        cleaned = cleaned.replace("--", "-")
    return cleaned.strip("-")


def utc_date_bucket(column, dialect: str):
    """Dialect-aware UTC calendar-date bucket (§1.4 rule 7).

    PostgreSQL converts through ``timezone('UTC', …)`` so session timezone can
    never skew buckets; the SQLite unit harness stores UTC timestamps, so
    ``date()`` already yields UTC dates.
    """
    if dialect == "postgresql":
        return cast(func.timezone("UTC", column), Date)
    return func.date(column)


def _as_utc(value: datetime | None) -> datetime | None:
    """Normalize fetched timestamps: SQLite returns naive UTC datetimes."""
    if value is None:
        return None
    return value.replace(tzinfo=UTC) if value.tzinfo is None else value.astimezone(UTC)


def _as_date(value: Any) -> date:
    if isinstance(value, date) and not isinstance(value, datetime):
        return value
    if isinstance(value, datetime):
        return value.date()
    return date.fromisoformat(str(value)[:10])


def recognized_market_org_ids():
    """Organizations with ≥1 APPROVED Buyer/Supplier member, demo excluded."""
    return (
        select(distinct(User.organization_id))
        .where(
            User.organization_id.is_not(None),
            User.role.in_(_MARKET_ROLES),
            User.status == UserStatus.APPROVED,
            User.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        )
    )


def order_provenance_case():
    recognized = recognized_market_org_ids()
    return case(
        (OrderBookOrder.organization_id.in_(list(DEMO_MARKET_ORG_IDS)), "demo"),
        (OrderBookOrder.organization_id.in_(recognized), "live"),
        else_="unknown",
    )


def trade_provenance_case():
    """Mixed live/demo trades are demo-contaminated, never live (rule 14)."""
    recognized = recognized_market_org_ids()
    demo_ids = list(DEMO_MARKET_ORG_IDS)
    return case(
        (or_(Trade.buyer_id.in_(demo_ids), Trade.seller_id.in_(demo_ids)), "demo"),
        (
            and_(Trade.buyer_id.in_(recognized), Trade.seller_id.in_(recognized)),
            "live",
        ),
        else_="unknown",
    )


def _in_window(column, start: datetime | None, end: datetime | None):
    if start is None or end is None:
        return literal(False)
    return and_(column.is_not(None), column >= start, column < end)


def _count_if(condition):
    return func.coalesce(func.sum(case((condition, 1), else_=0)), 0)


def _sum_if(condition, value_column):
    return func.coalesce(func.sum(case((condition, value_column), else_=0)), 0)


# ---------------------------------------------------------------------------
# Login-day fact writes (plan §2.4)
# ---------------------------------------------------------------------------


def is_retryable_transaction_error(error: Exception) -> bool:
    """Serialization/deadlock failures may be retried once by the caller."""
    orig = getattr(error, "orig", None)
    sqlstate = getattr(orig, "sqlstate", None) or getattr(orig, "pgcode", None)
    return sqlstate in {"40001", "40P01"}


async def record_login_day(db: AsyncSession, user: User, *, at: datetime | None = None) -> None:
    """Upsert one login-day row inside the caller's transaction.

    One atomic ``INSERT … ON CONFLICT (activity_date, user_id) DO UPDATE``
    increments ``login_count``, keeps the earliest ``first_login_at``, and
    advances ``last_login_at``. Sharing the authentication transaction means
    a database failure behaves exactly like the existing ``last_login``
    update — no best-effort side channel. Never stores IP, user agent,
    token, or credential data.
    """
    from uuid import uuid4

    from app.models.product_analytics import UserLoginDay

    instant = (at or datetime.now(UTC)).astimezone(UTC)
    dialect = db.get_bind().dialect.name
    if dialect == "postgresql":
        from sqlalchemy.dialects.postgresql import insert as upsert_insert

        earliest = func.least
        latest = func.greatest
    else:
        from sqlalchemy.dialects.sqlite import insert as upsert_insert

        # SQLite lacks LEAST/GREATEST; its two-argument MIN/MAX scalar
        # functions are the equivalent.
        earliest = func.min
        latest = func.max

    stmt = upsert_insert(UserLoginDay).values(
        id=uuid4(),
        activity_date=instant.date(),
        user_id=user.id,
        organization_id=user.organization_id,
        role=user.role,
        login_count=1,
        first_login_at=instant,
        last_login_at=instant,
    )
    stmt = stmt.on_conflict_do_update(
        index_elements=["activity_date", "user_id"],
        set_={
            "login_count": UserLoginDay.login_count + 1,
            "first_login_at": earliest(UserLoginDay.first_login_at, stmt.excluded.first_login_at),
            "last_login_at": latest(UserLoginDay.last_login_at, stmt.excluded.last_login_at),
        },
    )
    await db.execute(stmt)


# ---------------------------------------------------------------------------
# Statement builders (pure; reused by scripts/explain_product_analytics.py)
# ---------------------------------------------------------------------------


def registered_users_stmt(start: datetime, end: datetime,
                          previous_start: datetime | None, previous_end: datetime | None):
    """Buyer/Supplier registrations grouped by role, demo-org users excluded."""
    return (
        select(
            User.role.label("role"),
            _count_if(_in_window(User.created_at, start, end)).label("current"),
            _count_if(_in_window(User.created_at, previous_start, previous_end)).label("previous"),
        )
        .where(
            User.role.in_(_MARKET_ROLES),
            or_(
                User.organization_id.is_(None),
                User.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
            ),
            _in_window(User.created_at, previous_start or start, end),
        )
        .group_by(User.role)
    )


def _date_window(window_start: datetime | None, window_end: datetime | None):
    """Half-open date bounds for date-only facts projected to 00:00:00Z.

    A midday start excludes that date; a midday end includes it (§1.5).
    Returns ``(lower_inclusive, upper_exclusive)`` or ``None`` when the
    window is absent.
    """
    if window_start is None or window_end is None:
        return None
    lower = window_start.date()
    if window_start != datetime(lower.year, lower.month, lower.day, tzinfo=UTC):
        lower = lower + timedelta(days=1)
    upper = window_end.date()
    if window_end != datetime(upper.year, upper.month, upper.day, tzinfo=UTC):
        upper = upper + timedelta(days=1)
    return lower, upper


def _member_login_rows():
    """Base filter for member login-day facts (demo and admin excluded)."""
    from app.models.product_analytics import UserLoginDay

    return and_(
        UserLoginDay.role.in_(_MARKET_ROLES),
        or_(
            UserLoginDay.organization_id.is_(None),
            UserLoginDay.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        ),
    )


def users_facts_stmt(start: datetime, end: datetime,
                     previous_start: datetime | None, previous_end: datetime | None):
    """Registrations by role plus activation drop-off buckets in one round
    trip (UNION ALL over the users table)."""
    member = and_(
        User.role.in_(_MARKET_ROLES),
        or_(
            User.organization_id.is_(None),
            User.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        ),
    )
    registered = (
        select(
            literal("registered").label("kind"),
            cast(User.role, String).label("key"),
            _count_if(_in_window(User.created_at, start, end)).label("current"),
            _count_if(_in_window(User.created_at, previous_start, previous_end)).label(
                "previous"
            ),
        )
        .where(member, _in_window(User.created_at, previous_start or start, end))
        .group_by(User.role)
    )
    rejected = User.status == UserStatus.REJECTED
    unverified = and_(~rejected, User.email_verified.is_(False))
    pending = and_(~rejected, User.email_verified.is_(True), User.status == UserStatus.PENDING)
    org_incomplete = and_(
        User.status == UserStatus.APPROVED,
        User.email_verified.is_(True),
        User.organization_id.is_(None),
    )
    never_logged_in = and_(
        User.status == UserStatus.APPROVED,
        User.email_verified.is_(True),
        User.organization_id.is_not(None),
        User.last_login.is_(None),
    )
    drop_off_arms = [
        select(
            literal("drop_off").label("kind"),
            literal(key).label("key"),
            _count_if(condition).label("current"),
            literal(0).label("previous"),
        ).where(member)
        for key, condition in (
            ("rejected", rejected),
            ("unverified", unverified),
            ("pending_approval", pending),
            ("organization_incomplete", org_incomplete),
            ("never_logged_in", never_logged_in),
        )
    ]
    return union_all(registered, *drop_off_arms)


def login_day_facts_stmt(start: datetime, end: datetime,
                         previous_start: datetime | None, previous_end: datetime | None):
    """Member login-day series and summary in one round trip.

    The daily arm feeds the active-members trend; the summary arm carries
    window totals, DAU/WAU/MAU as-of end, and the login-history coverage
    start (the earliest fact row of any role, per §2.4).
    """
    from app.models.product_analytics import UserLoginDay

    current = _date_window(start, end)
    previous = _date_window(previous_start, previous_end)
    member = _member_login_rows()

    def in_dates(bounds):
        if bounds is None:
            return literal(False)
        lower, upper = bounds
        return and_(UserLoginDay.activity_date >= lower, UserLoginDay.activity_date < upper)

    assert current is not None
    upper = current[1]
    daily = (
        select(
            literal("daily").label("kind"),
            cast(UserLoginDay.activity_date, String).label("day"),
            func.count(distinct(UserLoginDay.user_id)).label("active_current"),
            literal(0).label("active_previous"),
            literal(0).label("dau"),
            literal(0).label("wau"),
            literal(0).label("mau"),
            cast(null(), String).label("coverage_start"),
        )
        .where(member, in_dates(current))
        .group_by(UserLoginDay.activity_date)
    )
    summary = select(
        literal("summary").label("kind"),
        cast(null(), String).label("day"),
        func.count(distinct(case((in_dates(current), UserLoginDay.user_id)))).label(
            "active_current"
        ),
        func.count(distinct(case((in_dates(previous), UserLoginDay.user_id)))).label(
            "active_previous"
        ),
        func.count(
            distinct(
                case(
                    (UserLoginDay.activity_date >= upper - timedelta(days=1), UserLoginDay.user_id)
                )
            )
        ).label("dau"),
        func.count(
            distinct(
                case(
                    (UserLoginDay.activity_date >= upper - timedelta(days=7), UserLoginDay.user_id)
                )
            )
        ).label("wau"),
        func.count(
            distinct(
                case(
                    (UserLoginDay.activity_date >= upper - timedelta(days=30), UserLoginDay.user_id)
                )
            )
        ).label("mau"),
        cast(func.min(UserLoginDay.activity_date), String).label("coverage_start"),
    ).where(member, UserLoginDay.activity_date < upper)
    return union_all(daily, summary)


def status_transition_facts_stmt(start: datetime, end: datetime,
                                 previous_start: datetime | None, previous_end: datetime | None):
    """Approval-journey and as-of qualification aggregates in one round trip.

    Qualified organizations (§1.5): distinct non-demo organizations whose
    latest member transition at or before the as-of instant is APPROVED —
    reconstructed from the append-only history, never from ``User.status``.
    """
    from app.models.product_analytics import UserStatusTransition as T

    member = and_(
        T.role.in_(_MARKET_ROLES),
        or_(
            T.organization_id.is_(None),
            T.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        ),
    )

    def qualified_as_of(as_of: datetime | None):
        if as_of is None:
            return cast(null(), Integer)
        latest = (
            select(T.user_id.label("user_id"), func.max(T.effective_at).label("latest_at"))
            .where(T.effective_at <= as_of)
            .group_by(T.user_id)
            .subquery()
        )
        return (
            select(func.count(distinct(T.organization_id)))
            .select_from(T)
            .join(
                latest,
                and_(T.user_id == latest.c.user_id, T.effective_at == latest.c.latest_at),
            )
            .where(
                T.to_status == UserStatus.APPROVED,
                T.organization_id.is_not(None),
                member,
            )
            .scalar_subquery()
        )

    approved = T.to_status == UserStatus.APPROVED
    return select(
        _count_if(and_(approved, member, _in_window(T.effective_at, start, end))).label(
            "approved_current"
        ),
        _count_if(
            and_(approved, member, _in_window(T.effective_at, previous_start, previous_end))
        ).label("approved_previous"),
        qualified_as_of(end).label("qualified_end"),
        qualified_as_of(previous_end).label("qualified_previous_end"),
        select(func.min(T.effective_at)).scalar_subquery().label("coverage_start"),
    )


def member_login_rollup_stmt(start: datetime, end: datetime,
                             previous_start: datetime | None, previous_end: datetime | None):
    """Per-member first-login instant, window flags, and registration instant.

    One grouped statement (bounded by member count) feeding returning-member
    counts, the first-login activation stage, and the registration→first-login
    distribution.
    """
    from app.models.product_analytics import UserLoginDay

    current = _date_window(start, end)
    previous = _date_window(previous_start, previous_end)

    def in_dates(bounds):
        if bounds is None:
            return literal(False)
        lower, upper = bounds
        return and_(UserLoginDay.activity_date >= lower, UserLoginDay.activity_date < upper)

    per_user = (
        select(
            UserLoginDay.user_id.label("user_id"),
            func.min(UserLoginDay.first_login_at).label("first_login_at"),
            func.min(UserLoginDay.activity_date).label("first_login_date"),
            func.max(case((in_dates(current), 1), else_=0)).label("in_current"),
            func.max(case((in_dates(previous), 1), else_=0)).label("in_previous"),
        )
        .where(_member_login_rows())
        .group_by(UserLoginDay.user_id)
        .subquery()
    )
    return select(
        per_user.c.user_id,
        per_user.c.first_login_at,
        per_user.c.first_login_date,
        per_user.c.in_current,
        per_user.c.in_previous,
        User.created_at.label("registered_at"),
    ).join(User, User.id == per_user.c.user_id)


def member_activity_days_stmt(end: datetime):
    """Distinct member login dates before the as-of instant (weekly cohort
    input; bounded by the 800-date retention window)."""
    from app.models.product_analytics import UserLoginDay

    upper = _date_window(datetime(1970, 1, 1, tzinfo=UTC), end)[1]
    return (
        select(
            UserLoginDay.user_id.label("user_id"),
            UserLoginDay.activity_date.label("activity_date"),
        )
        .where(_member_login_rows(), UserLoginDay.activity_date < upper)
        .distinct()
    )


def orders_aggregate_stmt(start: datetime, end: datetime,
                          previous_start: datetime | None, previous_end: datetime | None):
    """Order counts/sides/organizations/execution grouped by provenance.

    Execution (§1.5): an order created in the period counts as executed when a
    linked CONFIRMED/DELIVERED/PAID trade has ``confirmed_at`` strictly before
    the period end (as-of semantics; later fills never improve a closed
    period). Legacy trades without an order link never enter the rate.
    """
    provenance = order_provenance_case().label("provenance")
    current = _in_window(OrderBookOrder.created_at, start, end)
    previous = _in_window(OrderBookOrder.created_at, previous_start, previous_end)

    def executed_before(as_of: datetime):
        return exists(
            select(literal(1))
            .select_from(Trade)
            .where(
                or_(
                    Trade.bid_order_id == OrderBookOrder.id,
                    Trade.ask_order_id == OrderBookOrder.id,
                ),
                Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
                Trade.confirmed_at.is_not(None),
                Trade.confirmed_at < as_of,
            )
        )

    executed_previous = (
        _count_if(and_(previous, executed_before(previous_end)))
        if previous_start is not None and previous_end is not None
        else literal(0)
    )
    return (
        select(
            provenance,
            _count_if(current).label("orders_current"),
            _count_if(previous).label("orders_previous"),
            _count_if(and_(current, OrderBookOrder.side == OrderSide.BID)).label("bids_current"),
            _count_if(and_(current, OrderBookOrder.side == OrderSide.ASK)).label("asks_current"),
            func.count(distinct(case((current, OrderBookOrder.organization_id)))).label(
                "orgs_current"
            ),
            func.count(distinct(case((previous, OrderBookOrder.organization_id)))).label(
                "orgs_previous"
            ),
            func.count(
                distinct(
                    case(
                        (
                            and_(current, OrderBookOrder.side == OrderSide.BID),
                            OrderBookOrder.organization_id,
                        )
                    )
                )
            ).label("bid_orgs_current"),
            func.count(
                distinct(
                    case(
                        (
                            and_(current, OrderBookOrder.side == OrderSide.ASK),
                            OrderBookOrder.organization_id,
                        )
                    )
                )
            ).label("ask_orgs_current"),
            _count_if(and_(current, executed_before(end))).label("executed_current"),
            executed_previous.label("executed_previous"),
            _count_if(
                and_(
                    current,
                    OrderBookOrder.status.in_(_OPEN_ORDER_STATUSES),
                    OrderBookOrder.remaining_quantity_mt > 0,
                    or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at >= end),
                )
            ).label("still_open_current"),
        )
        .where(_in_window(OrderBookOrder.created_at, previous_start or start, end))
        .group_by(provenance)
    )


def orders_daily_stmt(start: datetime, end: datetime, dialect: str):
    """Daily live order counts for the current period (dense series built in
    Python)."""
    bucket = utc_date_bucket(OrderBookOrder.created_at, dialect).label("day")
    return (
        select(bucket, func.count(OrderBookOrder.id).label("orders"))
        .where(
            _in_window(OrderBookOrder.created_at, start, end),
            OrderBookOrder.organization_id.in_(recognized_market_org_ids()),
        )
        .group_by(bucket)
    )


def trades_aggregate_stmt(start: datetime, end: datetime,
                          previous_start: datetime | None, previous_end: datetime | None):
    """Confirmed-trade counts/volume/GMV and data-quality counters by provenance.

    Economic activity buckets by ``confirmed_at``; a legacy confirmed row with
    a null ``confirmed_at`` buckets by ``created_at`` and increments the
    legacy-fallback counter (§1.5). Realized GMV buckets PAID trades by
    ``paid_at``; PAID rows with a null ``paid_at`` are excluded and counted.
    """
    provenance = trade_provenance_case().label("provenance")
    confirmed = Trade.status.in_(_CONFIRMED_TRADE_STATUSES)
    strict_current = and_(confirmed, _in_window(Trade.confirmed_at, start, end))
    strict_previous = and_(confirmed, _in_window(Trade.confirmed_at, previous_start, previous_end))
    legacy_current = and_(
        confirmed, Trade.confirmed_at.is_(None), _in_window(Trade.created_at, start, end)
    )
    legacy_previous = and_(
        confirmed,
        Trade.confirmed_at.is_(None),
        _in_window(Trade.created_at, previous_start, previous_end),
    )
    economic_ts = func.coalesce(Trade.confirmed_at, Trade.created_at)
    volume = func.coalesce(Trade.final_quantity_mt, Trade.quantity_mt)
    paid = Trade.status == TradeStatus.PAID

    return (
        select(
            provenance,
            _count_if(strict_current).label("confirmed_current_strict"),
            _count_if(strict_previous).label("confirmed_previous_strict"),
            _count_if(legacy_current).label("legacy_current"),
            _count_if(legacy_previous).label("legacy_previous"),
            _sum_if(strict_current, volume).label("volume_current_strict"),
            _sum_if(legacy_current, volume).label("volume_current_legacy"),
            _sum_if(or_(strict_previous, legacy_previous), volume).label("volume_previous"),
            _sum_if(
                and_(paid, _in_window(Trade.paid_at, start, end)), Trade.final_total_usd
            ).label("gmv_current"),
            _sum_if(
                and_(paid, _in_window(Trade.paid_at, previous_start, previous_end)),
                Trade.final_total_usd,
            ).label("gmv_previous"),
            _count_if(
                and_(paid, Trade.paid_at.is_(None), _in_window(economic_ts, start, end))
            ).label("missing_paid_at_current"),
        )
        .where(
            confirmed,
            or_(
                _in_window(economic_ts, previous_start or start, end),
                and_(paid, _in_window(Trade.paid_at, previous_start or start, end)),
            ),
        )
        .group_by(provenance)
    )


def trades_daily_stmt(start: datetime, end: datetime, dialect: str):
    """Daily live confirmed-trade counts (legacy rows bucket by created_at)."""
    provenance = trade_provenance_case()
    economic_ts = func.coalesce(Trade.confirmed_at, Trade.created_at)
    bucket = utc_date_bucket(economic_ts, dialect).label("day")
    return (
        select(bucket, func.count(Trade.id).label("trades"))
        .where(
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
            _in_window(economic_ts, start, end),
            provenance == "live",
        )
        .group_by(bucket)
    )


def _org_activity_union(include_orders: bool = True):
    """Per-organization live activity instants: order creation and strict
    economic trade confirmation, one column layout for UNION ALL. The
    ``kind`` column distinguishes trade activity so trading-organization
    counts ride the same round trip."""
    recognized = recognized_market_org_ids()
    arms = []
    if include_orders:
        arms.append(
            select(
                OrderBookOrder.organization_id.label("org_id"),
                OrderBookOrder.created_at.label("ts"),
                literal("order").label("kind"),
            ).where(OrderBookOrder.organization_id.in_(recognized))
        )
    arms.append(
        select(
            Trade.buyer_id.label("org_id"),
            Trade.confirmed_at.label("ts"),
            literal("trade").label("kind"),
        ).where(
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
            Trade.confirmed_at.is_not(None),
            trade_provenance_case() == "live",
        )
    )
    arms.append(
        select(
            Trade.seller_id.label("org_id"),
            Trade.confirmed_at.label("ts"),
            literal("trade").label("kind"),
        ).where(
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
            Trade.confirmed_at.is_not(None),
            trade_provenance_case() == "live",
        )
    )
    return union_all(*arms).subquery("org_activity")


def org_activity_buckets_stmt(start: datetime, end: datetime,
                              previous_start: datetime | None, previous_end: datetime | None,
                              dialect: str):
    """Retention aggregates from live order/trade activity (§1.5).

    Retained: active in both the current and immediately preceding period.
    Reactivated: active now, inactive in the preceding period, active at
    least once before that. Repeat participation buckets organizations by
    distinct UTC activity days inside the current period.
    """
    activity = _org_activity_union()
    prev_prev_start = (
        previous_start - (previous_end - previous_start)
        if previous_start is not None and previous_end is not None
        else None
    )
    per_org = (
        select(
            activity.c.org_id,
            func.max(case((_in_window(activity.c.ts, start, end), 1), else_=0)).label("cur"),
            func.max(
                case((_in_window(activity.c.ts, previous_start, previous_end), 1), else_=0)
            ).label("prev"),
            func.max(
                case((_in_window(activity.c.ts, prev_prev_start, previous_start), 1), else_=0)
            ).label("prev_prev"),
            func.max(
                case(
                    (activity.c.ts < (prev_prev_start or previous_start or start), 1),
                    else_=0,
                )
            ).label("earlier"),
            func.max(
                case(
                    (
                        and_(activity.c.kind == "trade", _in_window(activity.c.ts, start, end)),
                        1,
                    ),
                    else_=0,
                )
            ).label("trading_cur"),
            func.max(
                case(
                    (
                        and_(
                            activity.c.kind == "trade",
                            _in_window(activity.c.ts, previous_start, previous_end),
                        ),
                        1,
                    ),
                    else_=0,
                )
            ).label("trading_prev"),
            func.count(
                distinct(
                    case(
                        (
                            _in_window(activity.c.ts, start, end),
                            utc_date_bucket(activity.c.ts, dialect),
                        )
                    )
                )
            ).label("active_days"),
        )
        .group_by(activity.c.org_id)
        .subquery("org_buckets")
    )
    b = per_org.c
    return select(
        _count_if(and_(b.cur == 1, b.prev == 1)).label("retained_current"),
        _count_if(and_(b.prev == 1, b.prev_prev == 1)).label("retained_previous"),
        _count_if(
            and_(b.cur == 1, b.prev == 0, or_(b.prev_prev == 1, b.earlier == 1))
        ).label("reactivated_current"),
        _count_if(and_(b.prev == 1, b.prev_prev == 0, b.earlier == 1)).label(
            "reactivated_previous"
        ),
        _count_if(b.trading_cur == 1).label("trading_current"),
        _count_if(b.trading_prev == 1).label("trading_previous"),
        _count_if(b.active_days == 1).label("days_1"),
        _count_if(b.active_days == 2).label("days_2"),
        _count_if(b.active_days >= 3).label("days_3_plus"),
    )


def eligible_quotes_stmt(as_of: datetime):
    """Eligible live/demo quote rows for slice liquidity (§1.6 Marketplace).

    An eligible quote is OPEN or PARTIALLY_FILLED with remaining quantity, a
    concrete delivery point, and no expiry before the as-of instant. Rows are
    classified by provenance; unknown-provenance quotes never enter liquidity.
    """
    provenance = order_provenance_case().label("provenance")
    return (
        select(
            provenance,
            OrderBookOrder.organization_id.label("org_id"),
            OrderBookOrder.side.label("side"),
            OrderBookOrder.price_per_mt_usd.label("price"),
            OrderBookOrder.remaining_quantity_mt.label("remaining"),
            OrderBookOrder.availability_window.label("window"),
            OrderBookOrder.created_at.label("created_at"),
            Product.name.label("product_name"),
            Product.fuel_type.label("fuel_type"),
            Product.fuel_grade.label("fuel_grade"),
            DeliveryPoint.name.label("delivery_point_name"),
        )
        .join(Product, Product.id == OrderBookOrder.product_id)
        .join(DeliveryPoint, DeliveryPoint.id == OrderBookOrder.delivery_point_id)
        .where(
            OrderBookOrder.status.in_(_OPEN_ORDER_STATUSES),
            OrderBookOrder.remaining_quantity_mt > 0,
            OrderBookOrder.delivery_point_id.is_not(None),
            or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at >= as_of),
            OrderBookOrder.created_at < as_of,
        )
    )


def matrix_window_stmt(start: datetime, end: datetime):
    """Live product×port matrix and availability-window distribution in one
    round trip (UNION ALL of two groupings)."""
    live = and_(
        _in_window(OrderBookOrder.created_at, start, end),
        OrderBookOrder.organization_id.in_(recognized_market_org_ids()),
    )
    matrix = (
        select(
            literal("matrix").label("kind"),
            Product.name.label("product_name"),
            Product.fuel_type.label("fuel_type"),
            Product.fuel_grade.label("fuel_grade"),
            DeliveryPoint.name.label("delivery_point_name"),
            cast(null(), String).label("window"),
            func.count(OrderBookOrder.id).label("orders"),
            func.count(distinct(OrderBookOrder.organization_id)).label("organizations"),
        )
        .join(Product, Product.id == OrderBookOrder.product_id)
        .join(DeliveryPoint, DeliveryPoint.id == OrderBookOrder.delivery_point_id)
        .where(live)
        .group_by(Product.name, Product.fuel_type, Product.fuel_grade, DeliveryPoint.name)
    )
    windows = (
        select(
            literal("window").label("kind"),
            cast(null(), String).label("product_name"),
            cast(null(), String).label("fuel_type"),
            cast(null(), String).label("fuel_grade"),
            cast(null(), String).label("delivery_point_name"),
            OrderBookOrder.availability_window.label("window"),
            func.count(OrderBookOrder.id).label("orders"),
            func.count(distinct(OrderBookOrder.organization_id)).label("organizations"),
        )
        .where(live)
        .group_by(OrderBookOrder.availability_window)
    )
    return union_all(matrix, windows)


def balance_trend_stmt(start: datetime, end: datetime, dialect: str):
    """Daily live order counts and distinct organizations per side."""
    bucket = utc_date_bucket(OrderBookOrder.created_at, dialect).label("day")
    return (
        select(
            bucket,
            OrderBookOrder.side.label("side"),
            func.count(OrderBookOrder.id).label("orders"),
            func.count(distinct(OrderBookOrder.organization_id)).label("organizations"),
        )
        .where(
            _in_window(OrderBookOrder.created_at, start, end),
            OrderBookOrder.organization_id.in_(recognized_market_org_ids()),
        )
        .group_by(bucket, OrderBookOrder.side)
    )


def status_distribution_stmt(start: datetime, end: datetime):
    """Order and trade status distributions by provenance in one round trip.

    The provenance expressions are shared between SELECT and GROUP BY —
    PostgreSQL requires textually identical grouped expressions.
    """
    order_provenance = order_provenance_case().label("provenance")
    orders = (
        select(
            literal("order").label("kind"),
            order_provenance,
            cast(OrderBookOrder.status, String).label("status"),
            func.count(OrderBookOrder.id).label("count"),
            func.count(distinct(OrderBookOrder.organization_id)).label("organizations"),
        )
        .where(_in_window(OrderBookOrder.created_at, start, end))
        .group_by(order_provenance, OrderBookOrder.status)
    )
    economic_ts = func.coalesce(Trade.confirmed_at, Trade.created_at)
    trade_provenance = trade_provenance_case().label("provenance")
    trades = (
        select(
            literal("trade").label("kind"),
            trade_provenance,
            cast(Trade.status, String).label("status"),
            func.count(Trade.id).label("count"),
            # Distinct organization counts across two party columns are not
            # derivable in this grouped layout; suppression for trade cells
            # falls back to the event-count rule (§1.4 rule 12).
            cast(null(), Integer).label("organizations"),
        )
        .where(_in_window(economic_ts, start, end))
        .group_by(trade_provenance, Trade.status)
    )
    return union_all(orders, trades)


def commissions_stmt(start: datetime, end: datetime,
                     previous_start: datetime | None, previous_end: datetime | None):
    """Realized revenue by payment date plus outstanding commission totals.

    ``Commission.payment_date`` is date-only and projects to 00:00:00Z on that
    UTC date; the half-open period predicate applies to the projected instant
    (§1.5). PAID rows with no payment date are excluded and counted. Demo
    trades never produce commissions; rows linked to demo-contaminated trades
    are excluded defensively.
    """

    def projected_window(window_start: datetime | None, window_end: datetime | None):
        bounds = _date_window(window_start, window_end)
        if bounds is None:
            return literal(False)
        lower, upper = bounds
        return and_(
            Commission.payment_date.is_not(None),
            Commission.payment_date >= lower,
            Commission.payment_date < upper,
        )

    paid = Commission.status == CommissionStatus.PAID
    demo_ids = list(DEMO_MARKET_ORG_IDS)
    demo_trade = and_(
        Trade.id.is_not(None),
        or_(Trade.buyer_id.in_(demo_ids), Trade.seller_id.in_(demo_ids)),
    )
    return (
        select(
            _sum_if(and_(paid, projected_window(start, end)), Commission.amount_usd).label(
                "revenue_current"
            ),
            _sum_if(
                and_(paid, projected_window(previous_start, previous_end)),
                Commission.amount_usd,
            ).label("revenue_previous"),
            _count_if(and_(paid, Commission.payment_date.is_(None))).label(
                "missing_payment_date"
            ),
            _sum_if(Commission.status == CommissionStatus.PENDING, Commission.amount_usd).label(
                "pending_total"
            ),
            _sum_if(Commission.status == CommissionStatus.INVOICED, Commission.amount_usd).label(
                "invoiced_total"
            ),
        )
        .select_from(Commission)
        .outerjoin(Trade, Trade.id == Commission.trade_id)
        .where(~demo_trade)
    )


def time_to_fill_stmt(start: datetime, end: datetime):
    """Per-order first economic confirmation for live orders created in the
    period (bounded by orders in the period; grouped, not looped)."""
    return (
        select(
            OrderBookOrder.id.label("order_id"),
            OrderBookOrder.created_at.label("created_at"),
            func.min(Trade.confirmed_at).label("first_confirmed_at"),
        )
        .join(
            Trade,
            or_(Trade.bid_order_id == OrderBookOrder.id, Trade.ask_order_id == OrderBookOrder.id),
        )
        .where(
            _in_window(OrderBookOrder.created_at, start, end),
            OrderBookOrder.organization_id.in_(recognized_market_org_ids()),
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
            Trade.confirmed_at.is_not(None),
            Trade.confirmed_at < end,
        )
        .group_by(OrderBookOrder.id, OrderBookOrder.created_at)
    )


def drop_off_stmt():
    """Mutually exclusive activation drop-off buckets over the current
    snapshot of non-demo Buyer/Supplier users."""
    rejected = User.status == UserStatus.REJECTED
    unverified = and_(~rejected, User.email_verified.is_(False))
    pending = and_(~rejected, User.email_verified.is_(True), User.status == UserStatus.PENDING)
    org_incomplete = and_(
        User.status == UserStatus.APPROVED,
        User.email_verified.is_(True),
        User.organization_id.is_(None),
    )
    never_logged_in = and_(
        User.status == UserStatus.APPROVED,
        User.email_verified.is_(True),
        User.organization_id.is_not(None),
        User.last_login.is_(None),
    )
    return select(
        _count_if(rejected).label("rejected"),
        _count_if(unverified).label("unverified"),
        _count_if(pending).label("pending_approval"),
        _count_if(org_incomplete).label("organization_incomplete"),
        _count_if(never_logged_in).label("never_logged_in"),
    ).where(
        User.role.in_(_MARKET_ROLES),
        or_(
            User.organization_id.is_(None),
            User.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        ),
    )


def org_first_live_order_stmt():
    """First-ever live order instant per recognized organization, with the
    organization's creation instant (durations computed in Python)."""
    from app.models.user import Organization

    first_orders = (
        select(
            OrderBookOrder.organization_id.label("org_id"),
            func.min(OrderBookOrder.created_at).label("first_order_at"),
        )
        .where(OrderBookOrder.organization_id.in_(recognized_market_org_ids()))
        .group_by(OrderBookOrder.organization_id)
        .subquery("first_orders")
    )
    return select(
        first_orders.c.org_id,
        first_orders.c.first_order_at,
        Organization.created_at.label("org_created_at"),
    ).join(Organization, Organization.id == first_orders.c.org_id)


# ---------------------------------------------------------------------------
# Suppression and shaping helpers
# ---------------------------------------------------------------------------


@dataclass
class _QualityTracker:
    legacy_timestamp_fallback_count: int = 0
    missing_paid_at_count: int = 0
    missing_commission_payment_date_count: int = 0
    suppressed_cell_count: int = 0
    cohort_complete: bool = True

    def to_schema(self) -> AnalyticsDataQuality:
        return AnalyticsDataQuality(
            legacy_timestamp_fallback_count=self.legacy_timestamp_fallback_count,
            missing_paid_at_count=self.missing_paid_at_count,
            missing_commission_payment_date_count=self.missing_commission_payment_date_count,
            suppressed_cell_count=self.suppressed_cell_count,
            cohort_complete=self.cohort_complete,
        )


def _suppress(count: int | None, entities: int | None, quality: _QualityTracker) -> tuple[int | None, bool]:
    """Apply §1.4 rule 12: 0 < entities < 3 suppresses; zero stays visible.

    ``entities`` falls back to ``count`` (event-count rule) when distinct
    entity counts are unavailable.
    """
    if count is None:
        return None, False
    basis = entities if entities is not None else count
    if 0 < basis < _SUPPRESSION_THRESHOLD:
        quality.suppressed_cell_count += 1
        return None, True
    return count, False


def _cell(key: str, count: int | None, entities: int | None, quality: _QualityTracker) -> AggregateCell:
    value, suppressed = _suppress(count, entities, quality)
    return AggregateCell(key=key, count=value, suppressed=suppressed)


def _metric(value: int | None, previous: int | None, *, segmented: bool,
            quality: _QualityTracker) -> MetricValue:
    if not segmented:
        return MetricValue(value=value, previous=previous)
    shown_value, suppressed_value = _suppress(value, None, quality)
    shown_previous, suppressed_previous = _suppress(previous, None, quality)
    return MetricValue(
        value=shown_value,
        previous=shown_previous,
        suppressed=suppressed_value or suppressed_previous,
    )


def _dense_series(start: datetime, end: datetime, values: dict[date, int]) -> list[SeriesPoint]:
    days: list[SeriesPoint] = []
    cursor = start.date()
    while cursor < end.date() or (cursor == end.date() and end.time() != datetime.min.time()):
        days.append(SeriesPoint(date=cursor, value=values.get(cursor, 0)))
        cursor += timedelta(days=1)
        if len(days) >= 366:
            break
    return days


def _median_hours(deltas: list[timedelta]) -> Decimal | None:
    if not deltas:
        return None
    hours = [delta.total_seconds() / 3600 for delta in deltas]
    return Decimal(str(statistics.median(hours))).quantize(Decimal("0.1"))


def _money(value: Any) -> Decimal:
    return Decimal(str(value or 0)).quantize(Decimal("0.01"))


def _quantity(value: Any) -> Decimal:
    return Decimal(str(value or 0)).quantize(Decimal("0.01"))


def _rate_pct(numerator: int, denominator: int) -> Decimal | None:
    if denominator <= 0:
        return None
    return (
        Decimal(numerator) / Decimal(denominator) * Decimal(100)
    ).quantize(Decimal("0.01"), rounding=ROUND_HALF_UP)


# ---------------------------------------------------------------------------
# Result containers
# ---------------------------------------------------------------------------


@dataclass
class LifecycleCounts:
    registered: MetricValue
    active: MetricValue
    participating: MetricValue
    trading: MetricValue
    retained: MetricValue


@dataclass
class OverviewAuthoritative:
    kpis: OverviewKpis
    lifecycle: LifecycleCounts
    orders_series: list[SeriesPoint]
    confirmed_trades_series: list[SeriesPoint]
    active_members_series: list[SeriesPoint]
    marketplace_balance: MarketplaceBalance
    one_sided_live_market: bool
    dormant_approved_members: int
    login_coverage_start: datetime | None
    status_coverage_start: datetime | None
    data_quality: AnalyticsDataQuality


@dataclass
class MarketplaceAuthoritative:
    live: MarketActivitySection | None
    demo: MarketActivitySection | None
    unknown: MarketActivitySection | None
    reference: ReferenceCoverageSection | None
    commercial: CommercialSummary | None
    data_quality: AnalyticsDataQuality


@dataclass
class ActivationAuthoritative:
    registered_total: MetricValue
    registered_buyer: AggregateCell
    registered_supplier: AggregateCell
    approved_members: AggregateCell
    first_login_members: AggregateCell
    first_live_order_organizations: AggregateCell
    drop_off: list[AggregateCell]
    time_to_first_login: DurationDistribution
    time_to_first_live_order: DurationDistribution
    login_coverage_start: datetime | None
    status_coverage_start: datetime | None
    data_quality: AnalyticsDataQuality


@dataclass
class RetentionAuthoritative:
    returning_members: MetricValue
    retained_organizations: MetricValue
    reactivated_organizations: MetricValue
    dormant_approved_members: MetricValue
    repeat_participation: list[AggregateCell]
    member_cohorts: list["CohortRow"]
    login_coverage_start: datetime | None
    data_quality: AnalyticsDataQuality


@dataclass
class EngagementAuthoritative:
    kpis: EngagementKpis
    active_members_trend: list[SeriesPoint]
    login_coverage_start: datetime | None
    data_quality: AnalyticsDataQuality


@dataclass
class _LoginFacts:
    coverage_start: date | None
    active_current: int
    active_previous: int
    dau: int
    wau: int
    mau: int
    daily: dict[date, int]


# ---------------------------------------------------------------------------
# Service
# ---------------------------------------------------------------------------


class ProductAnalyticsService:
    def __init__(self, db: AsyncSession):
        self.db = db

    @property
    def _dialect(self) -> str:
        bind = self.db.get_bind()
        return bind.dialect.name if bind is not None else "postgresql"

    # -- Overview ----------------------------------------------------------

    async def overview(self, query: ProductAnalyticsQuery) -> OverviewAuthoritative:
        quality = _QualityTracker()
        start, end = query.start, query.end
        prev_start, prev_end = query.previous_start, query.previous_end

        users_rows = (
            await self.db.execute(users_facts_stmt(start, end, prev_start, prev_end))
        ).all()
        orders_rows = (
            await self.db.execute(orders_aggregate_stmt(start, end, prev_start, prev_end))
        ).all()
        daily_orders = (await self.db.execute(orders_daily_stmt(start, end, self._dialect))).all()
        trades_rows = (
            await self.db.execute(trades_aggregate_stmt(start, end, prev_start, prev_end))
        ).all()
        daily_trades = (await self.db.execute(trades_daily_stmt(start, end, self._dialect))).all()
        activity = (
            await self.db.execute(
                org_activity_buckets_stmt(start, end, prev_start, prev_end, self._dialect)
            )
        ).one()
        login = self._parse_login_facts(
            (
                await self.db.execute(
                    login_day_facts_stmt(start, end, prev_start, prev_end)
                )
            ).all()
        )
        status_facts = (
            await self.db.execute(
                status_transition_facts_stmt(start, end, prev_start, prev_end)
            )
        ).one()

        registered_rows = [row for row in users_rows if row.kind == "registered"]
        drop_off_rows = {row.key: row.current for row in users_rows if row.kind == "drop_off"}
        registered_current = sum(row.current for row in registered_rows)
        registered_previous = sum(row.previous for row in registered_rows)
        live_orders = self._provenance_row(orders_rows, "live")
        live_trades = self._provenance_row(trades_rows, "live")

        confirmed_current = confirmed_previous = 0
        if live_trades is not None:
            quality.legacy_timestamp_fallback_count = (
                live_trades.legacy_current + live_trades.legacy_previous
            )
            quality.missing_paid_at_count = live_trades.missing_paid_at_current
            confirmed_current = live_trades.confirmed_current_strict + live_trades.legacy_current
            confirmed_previous = (
                live_trades.confirmed_previous_strict + live_trades.legacy_previous
            )

        orders_current = live_orders.orders_current if live_orders else 0
        orders_previous = live_orders.orders_previous if live_orders else 0
        participating_current = live_orders.orgs_current if live_orders else 0
        participating_previous = live_orders.orgs_previous if live_orders else 0
        bids = live_orders.bids_current if live_orders else 0
        asks = live_orders.asks_current if live_orders else 0

        active_members = self._login_metric(query, login)
        status_coverage = _as_utc(status_facts.coverage_start)
        qualified = MetricValue(
            value=(
                status_facts.qualified_end
                if status_coverage is not None and end >= status_coverage
                else None
            ),
            previous=(
                status_facts.qualified_previous_end
                if prev_end is not None
                and status_coverage is not None
                and prev_end >= status_coverage
                else None
            ),
        )

        kpis = OverviewKpis(
            qualified_organizations=qualified,
            active_members=active_members,
            participating_organizations=MetricValue(
                value=participating_current, previous=participating_previous
            ),
            live_orders=MetricValue(value=orders_current, previous=orders_previous),
            confirmed_trades=MetricValue(value=confirmed_current, previous=confirmed_previous),
        )
        lifecycle = LifecycleCounts(
            registered=MetricValue(value=registered_current, previous=registered_previous),
            active=active_members,
            participating=kpis.participating_organizations,
            trading=MetricValue(
                value=activity.trading_current, previous=activity.trading_previous
            ),
            retained=MetricValue(
                value=activity.retained_current, previous=activity.retained_previous
            ),
        )
        balance = MarketplaceBalance(
            buyer_organizations=_metric(
                live_orders.bid_orgs_current if live_orders else 0,
                None,
                segmented=True,
                quality=quality,
            ),
            supplier_organizations=_metric(
                live_orders.ask_orgs_current if live_orders else 0,
                None,
                segmented=True,
                quality=quality,
            ),
            bid_orders=MetricValue(value=bids),
            ask_orders=MetricValue(value=asks),
        )
        return OverviewAuthoritative(
            kpis=kpis,
            lifecycle=lifecycle,
            orders_series=self._series(daily_orders, "orders", start, end),
            confirmed_trades_series=self._series(daily_trades, "trades", start, end),
            active_members_series=_dense_series(start, end, login.daily),
            marketplace_balance=balance,
            one_sided_live_market=(bids == 0) != (asks == 0),
            dormant_approved_members=drop_off_rows.get("never_logged_in", 0),
            login_coverage_start=self._coverage_instant(login.coverage_start),
            status_coverage_start=status_coverage,
            data_quality=quality.to_schema(),
        )

    @staticmethod
    def _parse_login_facts(rows) -> _LoginFacts:
        coverage: date | None = None
        active_current = active_previous = dau = wau = mau = 0
        daily: dict[date, int] = {}
        for row in rows:
            if row.kind == "summary":
                active_current = row.active_current
                active_previous = row.active_previous
                dau, wau, mau = row.dau, row.wau, row.mau
                coverage = _as_date(row.coverage_start) if row.coverage_start else None
            else:
                daily[_as_date(row.day)] = row.active_current
        return _LoginFacts(
            coverage_start=coverage,
            active_current=active_current,
            active_previous=active_previous,
            dau=dau,
            wau=wau,
            mau=mau,
            daily=daily,
        )

    @staticmethod
    def _coverage_instant(coverage_date: date | None) -> datetime | None:
        if coverage_date is None:
            return None
        return datetime(coverage_date.year, coverage_date.month, coverage_date.day, tzinfo=UTC)

    def _login_metric(self, query: ProductAnalyticsQuery, login: _LoginFacts) -> MetricValue:
        """Active-member counts, null when the login-history facts cannot
        cover the requested window (§2.4 — never fabricated from
        ``User.last_login``)."""
        current_bounds = _date_window(query.start, query.end)
        current_covered = (
            login.coverage_start is not None
            and current_bounds is not None
            and current_bounds[0] >= login.coverage_start
        )
        previous_bounds = _date_window(query.previous_start, query.previous_end)
        previous_covered = (
            login.coverage_start is not None
            and previous_bounds is not None
            and previous_bounds[0] >= login.coverage_start
        )
        return MetricValue(
            value=login.active_current if current_covered else None,
            previous=login.active_previous if previous_covered else None,
        )

    # -- Marketplace ---------------------------------------------------------

    async def marketplace(self, query: ProductAnalyticsQuery) -> MarketplaceAuthoritative:
        quality = _QualityTracker()
        start, end = query.start, query.end
        prev_start, prev_end = query.previous_start, query.previous_end
        activity = query.activity

        wants_activity = activity in (
            AnalyticsActivity.LIVE,
            AnalyticsActivity.DEMO,
            AnalyticsActivity.ALL,
        ) or activity == AnalyticsActivity.ALL
        wants_unknown = activity == AnalyticsActivity.ALL
        wants_reference = activity in (AnalyticsActivity.REFERENCE, AnalyticsActivity.ALL)

        live_section = demo_section = unknown_section = None
        commercial = None

        if wants_activity:
            orders_rows = (
                await self.db.execute(orders_aggregate_stmt(start, end, prev_start, prev_end))
            ).all()
            quotes_rows = (await self.db.execute(eligible_quotes_stmt(end))).all()
            trades_rows = (
                await self.db.execute(trades_aggregate_stmt(start, end, prev_start, prev_end))
            ).all()
            matrix_rows = (await self.db.execute(matrix_window_stmt(start, end))).all()
            trend_rows = (
                await self.db.execute(balance_trend_stmt(start, end, self._dialect))
            ).all()
            status_rows = (await self.db.execute(status_distribution_stmt(start, end))).all()
            fill_rows = (await self.db.execute(time_to_fill_stmt(start, end))).all()
            commissions = (
                await self.db.execute(commissions_stmt(start, end, prev_start, prev_end))
            ).one()

            sections: dict[str, MarketActivitySection] = {}
            for provenance in ("live", "demo", "unknown"):
                sections[provenance] = self._activity_section(
                    provenance,  # type: ignore[arg-type]
                    query=query,
                    orders_rows=orders_rows,
                    trades_rows=trades_rows,
                    quotes_rows=quotes_rows,
                    matrix_rows=matrix_rows,
                    trend_rows=trend_rows,
                    status_rows=status_rows,
                    fill_rows=fill_rows,
                    quality=quality,
                )
            if activity in (AnalyticsActivity.LIVE, AnalyticsActivity.ALL):
                live_section = sections["live"]
            if activity in (AnalyticsActivity.DEMO, AnalyticsActivity.ALL):
                demo_section = sections["demo"]
            if wants_unknown:
                unknown_section = sections["unknown"]

            live_trades = self._provenance_row(trades_rows, "live")
            quality.missing_commission_payment_date_count = commissions.missing_payment_date
            if live_trades is not None:
                quality.legacy_timestamp_fallback_count = (
                    live_trades.legacy_current + live_trades.legacy_previous
                )
                quality.missing_paid_at_count = live_trades.missing_paid_at_current
            if activity in (AnalyticsActivity.LIVE, AnalyticsActivity.ALL):
                commercial = CommercialSummary(
                    realized_gmv_usd=DecimalMetricValue(
                        value=_money(live_trades.gmv_current if live_trades else 0),
                        previous=_money(live_trades.gmv_previous if live_trades else 0),
                    ),
                    realized_revenue_usd=DecimalMetricValue(
                        value=_money(commissions.revenue_current),
                        previous=_money(commissions.revenue_previous),
                    ),
                    commission_pending_usd=_money(commissions.pending_total),
                    commission_invoiced_usd=_money(commissions.invoiced_total),
                )

        reference_section = None
        if wants_reference:
            reference_section = await self._reference_section(query)

        return MarketplaceAuthoritative(
            live=live_section,
            demo=demo_section,
            unknown=unknown_section,
            reference=reference_section,
            commercial=commercial,
            data_quality=quality.to_schema(),
        )

    # -- Activation ----------------------------------------------------------

    async def activation(self, query: ProductAnalyticsQuery) -> ActivationAuthoritative:
        quality = _QualityTracker()
        start, end = query.start, query.end
        prev_start, prev_end = query.previous_start, query.previous_end

        registered_rows = (
            await self.db.execute(registered_users_stmt(start, end, prev_start, prev_end))
        ).all()
        drop_off_row = (await self.db.execute(drop_off_stmt())).one()
        first_orders = (await self.db.execute(org_first_live_order_stmt())).all()

        by_role = {row.role: row for row in registered_rows}
        buyer_row = by_role.get(UserRole.BUYER)
        supplier_row = by_role.get(UserRole.SUPPLIER)
        registered_current = sum(row.current for row in registered_rows)
        registered_previous = sum(row.previous for row in registered_rows)

        durations: list[timedelta] = []
        for row in first_orders:
            first_at = _as_utc(row.first_order_at)
            if first_at is not None and start <= first_at < end:
                durations.append(first_at - _as_utc(row.org_created_at))
        first_order_orgs = len(durations)
        sample_suppressed = 0 < first_order_orgs < _SUPPRESSION_THRESHOLD
        if sample_suppressed:
            quality.suppressed_cell_count += 1

        drop_off = [
            _cell("rejected", drop_off_row.rejected, None, quality),
            _cell("unverified", drop_off_row.unverified, None, quality),
            _cell("pending_approval", drop_off_row.pending_approval, None, quality),
            _cell(
                "organization_incomplete",
                drop_off_row.organization_incomplete,
                None,
                quality,
            ),
            _cell("never_logged_in", drop_off_row.never_logged_in, None, quality),
        ]

        # Fact-backed approval journey and first-login stages (§2.4).
        status_facts = (
            await self.db.execute(
                status_transition_facts_stmt(start, end, prev_start, prev_end)
            )
        ).one()
        rollup_rows = (
            await self.db.execute(
                member_login_rollup_stmt(start, end, prev_start, prev_end)
            )
        ).all()
        status_coverage = _as_utc(status_facts.coverage_start)
        approved_available = status_coverage is not None and status_coverage <= end

        current_bounds = _date_window(start, end)
        first_login_durations: list[timedelta] = []
        login_coverage: date | None = None
        for row in rollup_rows:
            first_date = _as_date(row.first_login_date)
            login_coverage = (
                first_date if login_coverage is None else min(login_coverage, first_date)
            )
            if current_bounds and current_bounds[0] <= first_date < current_bounds[1]:
                first_login_durations.append(
                    _as_utc(row.first_login_at) - _as_utc(row.registered_at)
                )
        first_login_count = len(first_login_durations)
        first_login_suppressed = 0 < first_login_count < _SUPPRESSION_THRESHOLD
        if first_login_suppressed:
            quality.suppressed_cell_count += 1

        return ActivationAuthoritative(
            registered_total=MetricValue(
                value=registered_current, previous=registered_previous
            ),
            registered_buyer=_cell(
                "BUYER", buyer_row.current if buyer_row else 0, None, quality
            ),
            registered_supplier=_cell(
                "SUPPLIER", supplier_row.current if supplier_row else 0, None, quality
            ),
            approved_members=_cell(
                "approved",
                status_facts.approved_current if approved_available else None,
                None,
                quality,
            ),
            first_login_members=_cell("first_login", first_login_count, None, quality),
            first_live_order_organizations=_cell(
                "first_live_order", first_order_orgs, None, quality
            ),
            drop_off=drop_off,
            time_to_first_login=DurationDistribution(
                buckets=[],
                median_hours=(
                    None if first_login_suppressed else _median_hours(first_login_durations)
                ),
                sample_size=(
                    None if first_login_suppressed else (first_login_count or None)
                ),
                suppressed=first_login_suppressed,
            ),
            time_to_first_live_order=DurationDistribution(
                buckets=[],
                median_hours=None if sample_suppressed else _median_hours(durations),
                sample_size=None if sample_suppressed else (first_order_orgs or None),
                suppressed=sample_suppressed,
            ),
            login_coverage_start=self._coverage_instant(login_coverage),
            status_coverage_start=status_coverage,
            data_quality=quality.to_schema(),
        )

    # -- Engagement (login-fact backed portions) -------------------------------

    async def engagement(self, query: ProductAnalyticsQuery) -> EngagementAuthoritative:
        quality = _QualityTracker()
        start, end = query.start, query.end
        prev_start, prev_end = query.previous_start, query.previous_end
        login = self._parse_login_facts(
            (
                await self.db.execute(
                    login_day_facts_stmt(start, end, prev_start, prev_end)
                )
            ).all()
        )
        bounds = _date_window(start, end)
        upper = bounds[1] if bounds else None

        def rolling_metric(value: int, days: int) -> MetricValue:
            covered = (
                login.coverage_start is not None
                and upper is not None
                and upper - timedelta(days=days) >= login.coverage_start
            )
            return MetricValue(value=value if covered else None)

        dau = rolling_metric(login.dau, 1)
        wau = rolling_metric(login.wau, 7)
        mau = rolling_metric(login.mau, 30)
        stickiness = None
        if dau.value is not None and mau.value:
            stickiness = _rate_pct(dau.value, mau.value)
        return EngagementAuthoritative(
            kpis=EngagementKpis(dau=dau, wau=wau, mau=mau, stickiness_pct=stickiness),
            active_members_trend=_dense_series(start, end, login.daily),
            login_coverage_start=self._coverage_instant(login.coverage_start),
            data_quality=quality.to_schema(),
        )

    # -- Retention (order/trade and login-fact backed portions) ---------------

    async def retention(self, query: ProductAnalyticsQuery) -> RetentionAuthoritative:
        quality = _QualityTracker()
        start, end = query.start, query.end
        prev_start, prev_end = query.previous_start, query.previous_end
        activity = (
            await self.db.execute(
                org_activity_buckets_stmt(start, end, prev_start, prev_end, self._dialect)
            )
        ).one()
        dormant = (await self.db.execute(drop_off_stmt())).one()
        rollup_rows = (
            await self.db.execute(
                member_login_rollup_stmt(start, end, prev_start, prev_end)
            )
        ).all()
        activity_days = (await self.db.execute(member_activity_days_stmt(end))).all()

        login_coverage: date | None = None
        returning_count = 0
        for row in rollup_rows:
            first_date = _as_date(row.first_login_date)
            login_coverage = (
                first_date if login_coverage is None else min(login_coverage, first_date)
            )
            if row.in_current and row.in_previous:
                returning_count += 1
        previous_bounds = _date_window(prev_start, prev_end)
        returning_covered = (
            login_coverage is not None
            and previous_bounds is not None
            and previous_bounds[0] >= login_coverage
        )

        repeat = [
            _cell("1", activity.days_1, None, quality),
            _cell("2", activity.days_2, None, quality),
            _cell("3_plus", activity.days_3_plus, None, quality),
        ]
        return RetentionAuthoritative(
            returning_members=MetricValue(
                value=returning_count if returning_covered else None
            ),
            retained_organizations=MetricValue(
                value=activity.retained_current, previous=activity.retained_previous
            ),
            reactivated_organizations=MetricValue(
                value=activity.reactivated_current, previous=activity.reactivated_previous
            ),
            dormant_approved_members=MetricValue(value=dormant.never_logged_in),
            repeat_participation=repeat,
            member_cohorts=self._member_cohorts(activity_days, end, quality),
            login_coverage_start=self._coverage_instant(login_coverage),
            data_quality=quality.to_schema(),
        )

    def _member_cohorts(self, activity_days, end: datetime, quality: _QualityTracker) -> list[CohortRow]:
        """Weekly member login cohorts from the fact rows (§1.6 Retention).

        Cohort = ISO week (Monday-anchored) of a member's first recorded
        login day; cells count cohort members active in each subsequent
        week. History never predates login-fact coverage.
        """

        def week_start(day: date) -> date:
            return day - timedelta(days=day.weekday())

        per_user: dict[Any, set[date]] = {}
        for row in activity_days:
            per_user.setdefault(row.user_id, set()).add(week_start(_as_date(row.activity_date)))
        cohorts: dict[date, dict[int, int]] = {}
        sizes: dict[date, int] = {}
        for weeks in per_user.values():
            first = min(weeks)
            sizes[first] = sizes.get(first, 0) + 1
            buckets = cohorts.setdefault(first, {})
            for week in weeks:
                offset = (week - first).days // 7
                buckets[offset] = buckets.get(offset, 0) + 1

        rows: list[CohortRow] = []
        for cohort_start in sorted(cohorts)[:_MAX_COHORTS]:
            size_value, size_suppressed = _suppress(sizes[cohort_start], None, quality)
            cells: list[CohortCell] = []
            for offset in sorted(cohorts[cohort_start]):
                value, suppressed = _suppress(cohorts[cohort_start][offset], None, quality)
                pct = None
                if value is not None and size_value:
                    pct = _rate_pct(value, size_value)
                cells.append(
                    CohortCell(
                        offset=offset,
                        cell=AggregateCell(key=str(offset), count=value, suppressed=suppressed),
                        pct=pct,
                    )
                )
            rows.append(
                CohortRow(
                    cohort_start=cohort_start,
                    size=AggregateCell(
                        key=cohort_start.isoformat(),
                        count=size_value,
                        suppressed=size_suppressed,
                    ),
                    cells=cells[:54],
                )
            )
        return rows

    # -- Internals ------------------------------------------------------------

    @staticmethod
    def _provenance_row(rows, provenance: str):
        for row in rows:
            if row.provenance == provenance:
                return row
        return None

    def _series(self, rows, value_key: str, start: datetime, end: datetime) -> list[SeriesPoint]:
        values = {_as_date(row.day): getattr(row, value_key) for row in rows}
        return _dense_series(start, end, values)

    def _activity_section(
        self,
        provenance: Provenance,
        *,
        query: ProductAnalyticsQuery,
        orders_rows,
        trades_rows,
        quotes_rows,
        matrix_rows,
        trend_rows,
        status_rows,
        fill_rows,
        quality: _QualityTracker,
    ) -> MarketActivitySection:
        orders = self._provenance_row(orders_rows, provenance)
        trades = self._provenance_row(trades_rows, provenance)

        confirmed_current = confirmed_previous = 0
        volume_current = volume_previous = Decimal("0")
        if trades is not None:
            confirmed_current = trades.confirmed_current_strict + trades.legacy_current
            confirmed_previous = trades.confirmed_previous_strict + trades.legacy_previous
            volume_current = _quantity(trades.volume_current_strict) + _quantity(
                trades.volume_current_legacy
            )
            volume_previous = _quantity(trades.volume_previous)

        executed = orders.executed_current if orders else 0
        created = orders.orders_current if orders else 0
        still_open = orders.still_open_current if orders else 0
        now = datetime.now(UTC)
        cohort_complete = not (query.end > now and still_open > 0)
        if not cohort_complete:
            quality.cohort_complete = False

        kpis = MarketplaceKpis(
            participating_organizations=MetricValue(
                value=orders.orgs_current if orders else 0,
                previous=orders.orgs_previous if orders else 0,
            ),
            open_bids=MetricValue(value=orders.bids_current if orders else 0),
            open_asks=MetricValue(value=orders.asks_current if orders else 0),
            confirmed_trades=MetricValue(value=confirmed_current, previous=confirmed_previous),
            confirmed_volume_mt=DecimalMetricValue(
                value=volume_current, previous=volume_previous
            ),
            execution_rate=RatioValue(
                numerator=executed,
                denominator=created,
                rate_pct=_rate_pct(executed, created),
                cohort_complete=cohort_complete,
            ),
        )

        liquidity = self._liquidity(provenance, quotes_rows, fill_rows, query, quality)

        # Matrix, window distribution, balance trend are live-only surfaces.
        matrix_cells: list[ProductPortCell] = []
        window_rows: list[RankedRow] = []
        trend = MarketBalanceTrend()
        if provenance == "live":
            matrix_cells, window_rows = self._matrix_and_windows(matrix_rows, quality)
            trend = self._balance_trend(trend_rows, query)

        order_statuses: list[AggregateCell] = []
        trade_statuses: list[AggregateCell] = []
        for row in status_rows:
            if row.provenance != provenance:
                continue
            status_value = getattr(row.status, "value", row.status)
            cell = _cell(str(status_value), row.count, row.organizations, quality)
            if row.kind == "order":
                order_statuses.append(cell)
            else:
                trade_statuses.append(cell)

        concentration = self._concentration(provenance, orders, quality)

        return MarketActivitySection(
            kpis=kpis,
            liquidity=liquidity,
            balance_trend=trend,
            product_port_matrix=matrix_cells,
            window_distribution=window_rows,
            order_status_distribution=sorted(order_statuses, key=lambda c: c.key),
            trade_status_distribution=sorted(trade_statuses, key=lambda c: c.key),
            concentration=concentration,
        )

    def _liquidity(
        self,
        provenance: Provenance,
        quotes_rows,
        fill_rows,
        query: ProductAnalyticsQuery,
        quality: _QualityTracker,
    ) -> LiquiditySummary:
        # Group eligible quotes into exact slices. Unknown provenance never
        # contributes liquidity; demo depth stays inside the demo section's
        # own slice rows and is never merged into live figures.
        slices: dict[tuple[str, str, str], dict[str, Any]] = {}
        for row in quotes_rows:
            if row.provenance != provenance or provenance == "unknown":
                continue
            market_product = derive_market_product(row.product_name, row.fuel_type, row.fuel_grade)
            product_key = market_product.value if market_product else slugify(row.product_name)
            key = (product_key, slugify(row.delivery_point_name), row.window)
            entry = slices.setdefault(
                key,
                {
                    "product_label": row.product_name,
                    "delivery_point_label": row.delivery_point_name,
                    "orgs": set(),
                    "bids": [],
                    "asks": [],
                    "created": [],
                },
            )
            entry["orgs"].add(row.org_id)
            entry["created"].append(_as_utc(row.created_at))
            price = Decimal(str(row.price))
            remaining = Decimal(str(row.remaining))
            if row.side == OrderSide.BID:
                entry["bids"].append((price, remaining))
            else:
                entry["asks"].append((price, remaining))

        rows: list[SliceLiquidityRow] = []
        two_sided = one_sided = crossed = 0
        spreads_usd: list[Decimal] = []
        spreads_bps: list[Decimal] = []
        ages: list[timedelta] = []
        for (product_key, dp_key, window), entry in sorted(slices.items()):
            bids = entry["bids"]
            asks = entry["asks"]
            if bids and asks:
                two_sided += 1
            elif bids or asks:
                one_sided += 1
            ages.extend(query.end - created for created in entry["created"])

            org_count = len(entry["orgs"])
            suppressed = org_count < _SUPPRESSION_THRESHOLD
            best_bid = max((price for price, _ in bids), default=None)
            best_ask = min((price for price, _ in asks), default=None)
            spread = mid = spread_bps = None
            is_crossed = None
            if best_bid is not None and best_ask is not None:
                spread = best_ask - best_bid
                mid = (best_ask + best_bid) / 2
                if mid > 0:
                    spread_bps = (spread / mid * Decimal(10000)).quantize(
                        Decimal("0.01"), rounding=ROUND_HALF_UP
                    )
                is_crossed = spread < 0
                if is_crossed:
                    crossed += 1
                if not suppressed:
                    spreads_usd.append(spread)
                    if spread_bps is not None:
                        spreads_bps.append(spread_bps)

            if suppressed:
                quality.suppressed_cell_count += 1
                rows.append(
                    SliceLiquidityRow(
                        product_key=product_key,
                        product_label=entry["product_label"],
                        delivery_point_key=dp_key,
                        delivery_point_label=entry["delivery_point_label"],
                        availability_window=window,
                        availability_window_label=availability_window_display_label(window),
                        suppressed=True,
                    )
                )
                continue

            def depth(quotes, predicate) -> Decimal:
                return sum((qty for price, qty in quotes if predicate(price)), Decimal("0"))

            one_pct_bid = (
                depth(bids, lambda p: p >= best_bid * Decimal("0.99")) if best_bid else None
            )
            one_pct_ask = (
                depth(asks, lambda p: p <= best_ask * Decimal("1.01")) if best_ask else None
            )
            rows.append(
                SliceLiquidityRow(
                    product_key=product_key,
                    product_label=entry["product_label"],
                    delivery_point_key=dp_key,
                    delivery_point_label=entry["delivery_point_label"],
                    availability_window=window,
                    availability_window_label=availability_window_display_label(window),
                    contributing_organizations=org_count,
                    best_bid_usd_per_mt=best_bid,
                    best_ask_usd_per_mt=best_ask,
                    spread_usd_per_mt=spread,
                    spread_bps=spread_bps,
                    best_bid_depth_mt=depth(bids, lambda p: p == best_bid) if best_bid else None,
                    best_ask_depth_mt=depth(asks, lambda p: p == best_ask) if best_ask else None,
                    one_percent_bid_depth_mt=one_pct_bid,
                    one_percent_ask_depth_mt=one_pct_ask,
                    crossed=is_crossed,
                    suppressed=False,
                )
            )

        fills = [
            _as_utc(row.first_confirmed_at) - _as_utc(row.created_at)
            for row in fill_rows
            if row.first_confirmed_at is not None
        ]
        return LiquiditySummary(
            two_sided_slices=two_sided,
            one_sided_slices=one_sided,
            crossed_slices=crossed,
            median_spread_usd_per_mt=(
                Decimal(str(statistics.median(spreads_usd))).quantize(Decimal("0.01"))
                if spreads_usd
                else None
            ),
            median_spread_bps=(
                Decimal(str(statistics.median(spreads_bps))).quantize(Decimal("0.01"))
                if spreads_bps
                else None
            ),
            median_open_order_age_hours=_median_hours(ages),
            median_hours_to_first_fill=_median_hours(fills) if provenance == "live" else None,
            slices=rows[:20],
        )

    def _matrix_and_windows(
        self, matrix_rows, quality: _QualityTracker
    ) -> tuple[list[ProductPortCell], list[RankedRow]]:
        cells: list[ProductPortCell] = []
        windows: list[RankedRow] = []
        window_total = sum(row.orders for row in matrix_rows if row.kind == "window")
        for row in matrix_rows:
            if row.kind == "matrix":
                market_product = derive_market_product(
                    row.product_name, row.fuel_type, row.fuel_grade
                )
                product_key = (
                    market_product.value if market_product else slugify(row.product_name)
                )
                cells.append(
                    ProductPortCell(
                        product_key=product_key,
                        product_label=row.product_name,
                        delivery_point_key=slugify(row.delivery_point_name),
                        delivery_point_label=row.delivery_point_name,
                        orders=_cell("orders", row.orders, row.organizations, quality),
                        organizations=_cell(
                            "organizations", row.organizations, row.organizations, quality
                        ),
                    )
                )
            else:
                count, suppressed = _suppress(row.orders, row.organizations, quality)
                share = (
                    _rate_pct(row.orders, window_total)
                    if not suppressed and window_total
                    else None
                )
                windows.append(
                    RankedRow(
                        key=row.window,
                        label=availability_window_display_label(row.window),
                        count=count,
                        share_pct=share,
                        suppressed=suppressed,
                    )
                )
        cells.sort(key=lambda c: (c.product_key, c.delivery_point_key))
        windows.sort(key=lambda w: w.key)
        return cells[:512], windows[:20]

    def _balance_trend(self, trend_rows, query: ProductAnalyticsQuery) -> MarketBalanceTrend:
        buyer_orgs: dict[date, int] = {}
        supplier_orgs: dict[date, int] = {}
        bids: dict[date, int] = {}
        asks: dict[date, int] = {}
        for row in trend_rows:
            day = _as_date(row.day)
            if row.side == OrderSide.BID:
                buyer_orgs[day] = row.organizations
                bids[day] = row.orders
            else:
                supplier_orgs[day] = row.organizations
                asks[day] = row.orders
        return MarketBalanceTrend(
            buyer_organizations=_dense_series(query.start, query.end, buyer_orgs),
            supplier_organizations=_dense_series(query.start, query.end, supplier_orgs),
            bids=_dense_series(query.start, query.end, bids),
            asks=_dense_series(query.start, query.end, asks),
        )

    def _concentration(
        self, provenance: Provenance, orders_row, quality: _QualityTracker
    ) -> OrganizationConcentration:
        # HHI needs per-organization shares; publishing them is prohibited, so
        # the band derives from the participating-organization count alone
        # until a dedicated grouped share query is justified. Fewer than three
        # organizations is definitionally concentrated and always suppressed.
        orgs = orders_row.orgs_current if orders_row is not None else 0
        if 0 < orgs < _SUPPRESSION_THRESHOLD:
            quality.suppressed_cell_count += 1
            return OrganizationConcentration(hhi_band=None, suppressed=True)
        if orgs == 0:
            return OrganizationConcentration(hhi_band=None, suppressed=False)
        band = (
            ConcentrationBand.HIGH
            if orgs < 5
            else ConcentrationBand.MODERATE
            if orgs < 10
            else ConcentrationBand.LOW
        )
        return OrganizationConcentration(hhi_band=band, suppressed=False)

    async def _reference_section(self, query: ProductAnalyticsQuery) -> ReferenceCoverageSection:
        """Compose reference coverage from the canonical benchmark service.

        One catalog round trip enumerates the requested canonical cells; one
        benchmark round trip resolves override/seed quotes. Reference is
        coverage, never activity (§1.6).
        """
        products_stmt = (
            select(
                Product.id,
                Product.name,
                Product.fuel_type,
                Product.fuel_grade,
                DeliveryPoint.id.label("dp_id"),
                DeliveryPoint.name.label("dp_name"),
            )
            .join(DeliveryPoint, literal(True))
            .where(Product.is_active.is_(True), DeliveryPoint.is_active.is_(True))
        )
        if query.product_id is not None:
            products_stmt = products_stmt.where(Product.id == query.product_id)
        if query.delivery_point_id is not None:
            products_stmt = products_stmt.where(DeliveryPoint.id == query.delivery_point_id)
        cells = (await self.db.execute(products_stmt)).all()

        window = query.availability_window or "SPOT"
        requests = []
        cell_meta: dict[tuple[str, UUID, str], dict[str, str]] = {}
        for row in cells:
            market_product = derive_market_product(row.name, row.fuel_type, row.fuel_grade)
            if market_product is None:
                continue
            requests.append((market_product.value, row.dp_id, window, row.dp_name))
            cell_meta[(market_product.value, row.dp_id, window)] = {
                "product_label": row.name,
                "dp_name": row.dp_name,
            }

        quotes = await get_benchmark_quotes(self.db, requests)

        rows: list[ReferenceCoverageRow] = []
        for key, meta in sorted(
            cell_meta.items(), key=lambda item: (item[0][0], str(item[0][1]))
        ):
            quote = quotes.get(key)
            market_product, _dp_id, window_key = key
            base = {
                "product_key": market_product,
                "product_label": meta["product_label"],
                "delivery_point_key": slugify(meta["dp_name"]),
                "delivery_point_label": meta["dp_name"],
                "availability_window": window_key,
                "availability_window_label": availability_window_display_label(window_key),
            }
            if quote is None:
                rows.append(
                    ReferenceCoverageRow(
                        **base,
                        benchmark_price_usd_per_mt=None,
                        source_label=ReferenceSourceLabel.OTHER,
                        generated_at=datetime.now(UTC),
                        observed_at=None,
                        source_kind=ReferenceSourceKind.OTHER,
                        scope=ReferenceScope.OTHER,
                        coverage_status="unavailable",
                    )
                )
                continue
            if quote.source == "manual_override":
                label = ReferenceSourceLabel.MANUAL_OVERRIDE
                kind = ReferenceSourceKind.ADMIN_BENCHMARK
                scope = ReferenceScope.EXACT_SLICE
            elif quote.source == "seed_matrix":
                label = ReferenceSourceLabel.SEED_MATRIX
                kind = ReferenceSourceKind.SEEDED_BENCHMARK
                scope = ReferenceScope.WINDOW_ADJUSTED
            else:
                label = ReferenceSourceLabel.OTHER
                kind = ReferenceSourceKind.OTHER
                scope = ReferenceScope.OTHER
            status = "current" if quote.observed_at is not None else "stale"
            rows.append(
                ReferenceCoverageRow(
                    **base,
                    benchmark_price_usd_per_mt=quote.benchmark_price_per_mt_usd,
                    source_label=label,
                    generated_at=quote.generated_at,
                    observed_at=quote.observed_at,
                    source_kind=kind,
                    scope=scope,
                    coverage_status=status,
                )
            )
        return ReferenceCoverageSection(rows=rows[:512])
