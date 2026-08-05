"""Platform-wide analytics endpoints for the admin dashboard."""
import hashlib
import uuid as _uuid
from enum import IntEnum
from datetime import datetime, timedelta, UTC
# Aliased because the DailyStat field is itself named `date` — pydantic
# cannot resolve an annotation shadowed by its own field name.
from datetime import date as date_type
# The DailyStat field is named `date`, which shadows the type inside the
# class body — pydantic needs an unshadowed alias for the annotation.
from typing import Annotated, Optional

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from pydantic import BaseModel, Field
from slowapi.util import get_remote_address
from sqlalchemy import select, func, cast, Date, distinct, or_
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.models.orderbook import (
    OrderBookOrder, OrderBookStatus, Trade, TradeStatus,
)
from app.models.user import User, UserRole, UserStatus, Organization
from app.rate_limit import limiter
from app.schemas.errors import AUTH_RESPONSES
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import ADMIN_USER_REJECTED
from app.schemas.behavioral_analytics import (
    AuthoritativeUsage,
    BehavioralUsage,
    FunnelStage,
    ProductUsageResponse,
)
from app.services.behavioral_analytics import UmamiAnalyticsService, get_analytics_service
from app.services.demo_market import DEMO_MARKET_ORG_IDS
from app.services.user_status_transition import record_status_transition


# ---------------------------------------------------------------------------
# Response schemas
# ---------------------------------------------------------------------------

class OverviewResponse(BaseModel):
    total_users: int = Field(description="Count of real market users (BUYER/SUPPLIER roles only; admins and seeded demo accounts excluded).")
    active_users_7d: int = Field(description="Market users whose last login is within the past 7 days.")
    total_organizations: int = Field(description="Organizations with at least one real market user.")
    total_orders: int = Field(description="All orders ever placed by market organizations, regardless of status.")
    open_orders: int = Field(description="Orders currently OPEN or PARTIALLY_FILLED.")
    total_trades: int = Field(description="All trades between market organizations, in ANY status — includes pending, cancelled, and declined. Compare with confirmed_trades.")
    confirmed_trades: int = Field(description="Trades in CONFIRMED, DELIVERED, or PAID status — trades that economically happened.")
    total_volume_mt: float = Field(description="Sum of quantity_mt over CONFIRMED/DELIVERED/PAID trades only (same filter as confirmed_trades).")
    total_revenue_usd: float = Field(description="Sum of commission_amount_usd over PAID trades only — realized platform revenue.")
    total_gmv_usd: float = Field(description="Sum of final_total_usd over PAID trades only — realized gross merchandise value.")


class DailyStat(BaseModel):
    date: date_type = Field(description="UTC calendar day of the bucket.")
    orders_placed: int = Field(description="Orders created by market organizations that day, any status.")
    trades_executed: int = Field(description="CONFIRMED/DELIVERED/PAID trades created that day.")
    volume_mt: float = Field(description="Sum of quantity_mt over that day's confirmed trades.")
    gmv_usd: float = Field(description="Sum of final_total_usd over that day's confirmed trades (any payment status).")
    commission_usd: float = Field(description="Sum of commission_amount_usd over that day's confirmed trades (accrued, not necessarily paid).")


class AdminUserEntry(BaseModel):
    id: _uuid.UUID
    email: str
    first_name: Optional[str]
    last_name: Optional[str]
    role: str
    status: str
    created_at: datetime
    organization_id: Optional[_uuid.UUID]
    org_name: Optional[str]
    org_type: Optional[str]
    org_provenance: Optional[str]
    # Journey fields: how far this account has actually gotten.
    email_verified: bool = False
    must_change_password: bool = False
    last_login: Optional[datetime] = None
    org_has_orders: bool = False

    class Config:
        from_attributes = True


class AdminUsersResponse(BaseModel):
    items: list[AdminUserEntry]
    total: int


# ---------------------------------------------------------------------------
# Router
# ---------------------------------------------------------------------------

router = APIRouter(
    prefix="/admin/analytics",
    tags=["admin-analytics"],
    responses=AUTH_RESPONSES,
)


_MARKET_MEMBER_ROLES = [UserRole.BUYER, UserRole.SUPPLIER]

# Trade statuses considered economically committed ("trade actually happened").
# Used to filter volume / gmv / commission aggregations so dashboards reflect
# realized activity, not cancelled or pending-confirmation matches.
# Tuple rather than list so the constant cannot be mutated by future callers.
_CONFIRMED_TRADE_STATUSES = (
    TradeStatus.CONFIRMED,
    TradeStatus.DELIVERED,
    TradeStatus.PAID,
)


def _market_member_org_ids_subquery():
    """
    Organizations that have at least one real (BUYER/SUPPLIER) user.

    Excludes seeded/demo liquidity orgs that have no real member accounts,
    as well as admin-only orgs. Uses an explicit role whitelist so that
    NULL roles and any future non-market roles are also excluded.

    Note on trade filtering: callers use this subquery with AND semantics
    (both buyer AND seller must be market members). Decision (Sprint 3
    item 7, 2026-07-04): AND is kept deliberately — demo listings can no
    longer be traded at all (create_trade and the matching engine both
    reject demo orgs), so a real-vs-demo trade cannot exist and the
    dashboard reports strictly real activity.
    """
    return (
        select(distinct(User.organization_id))
        .where(
            User.organization_id.is_not(None),
            User.role.in_(_MARKET_MEMBER_ROLES),
        )
    )


def _per_token_rate_key(request: Request) -> str:
    """
    Rate-limit bucket keyed by bearer-token identity (per-session-per-user).

    The global `limiter` is configured with `key_func=get_remote_address`,
    which collapses several admins behind a single corporate NAT/VPN into
    one rate bucket. For admin endpoints we want per-user granularity so
    one heavy poller cannot starve the others.

    We hash the Authorization header directly — no JWT parsing, no signing
    verification (require_role still gates access). Different tokens map
    to different buckets; a token rotation resets the bucket (acceptable).
    Falls back to remote address when no Authorization header is present,
    so anonymous callers still get throttled.
    """
    auth = request.headers.get("Authorization", "")
    if auth:
        return "tok:" + hashlib.sha256(auth.encode("utf-8")).hexdigest()[:16]
    return get_remote_address(request)


def _conversion_rate(value: int, previous: int) -> float | None:
    if previous <= 0:
        return None
    return round(value / previous * 100, 1)


class ProductUsagePeriod(IntEnum):
    SEVEN_DAYS = 7
    THIRTY_DAYS = 30
    NINETY_DAYS = 90


@router.get("/product-usage", response_model=ProductUsageResponse)
@limiter.limit("30/minute", key_func=_per_token_rate_key)
async def get_product_usage(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    days: ProductUsagePeriod = Query(ProductUsagePeriod.THIRTY_DAYS),
):
    """Aggregated product usage without session identifiers or commercial data."""
    days_value = int(days)
    period_end = datetime.now(UTC)
    period_start = period_end - timedelta(days=days_value)
    market_roles = tuple(_MARKET_MEMBER_ROLES)

    registrations_q = await db.execute(
        select(func.count(User.id)).where(
            User.role.in_(market_roles),
            User.created_at >= period_start,
            User.created_at < period_end,
        )
    )
    logins_q = await db.execute(
        select(func.count(User.id)).where(
            User.role.in_(market_roles),
            User.last_login >= period_start,
            User.last_login < period_end,
        )
    )
    order_orgs_q = await db.execute(
        select(func.count(distinct(OrderBookOrder.organization_id))).where(
            OrderBookOrder.created_at >= period_start,
            OrderBookOrder.created_at < period_end,
            OrderBookOrder.organization_id.in_(_market_member_org_ids_subquery()),
            OrderBookOrder.organization_id.notin_(list(DEMO_MARKET_ORG_IDS)),
        )
    )
    authoritative = AuthoritativeUsage(
        registrations=registrations_q.scalar() or 0,
        users_logging_in=logins_q.scalar() or 0,
        order_placing_organizations=order_orgs_q.scalar() or 0,
    )

    aggregate = await analytics.get_aggregate(days_value)
    average_session_duration = (
        round(aggregate.total_time_seconds / aggregate.visits, 1)
        if aggregate.visits
        else 0.0
    )
    behavioral = BehavioralUsage(
        visitors=aggregate.visitors,
        visits=aggregate.visits,
        pageviews=aggregate.pageviews,
        total_time_seconds=aggregate.total_time_seconds,
        average_session_duration_seconds=average_session_duration,
        event_totals=aggregate.event_totals,
        event_series=aggregate.event_series,
        daily_visitors=aggregate.daily_visitors,
        top_entries=aggregate.top_entries,
        top_referrers=aggregate.top_referrers,
    )

    stage_values = [
        ("visitors", behavioral.visitors),
        ("signup_started", behavioral.event_totals.get("signup_started", 0)),
        ("registrations", authoritative.registrations),
        ("users_logging_in", authoritative.users_logging_in),
        ("order_placing_organizations", authoritative.order_placing_organizations),
    ]
    funnel = [
        FunnelStage(
            name=name,
            value=value,
            conversion_from_previous_pct=(
                None if index == 0 else _conversion_rate(value, stage_values[index - 1][1])
            ),
        )
        for index, (name, value) in enumerate(stage_values)
    ]

    return ProductUsageResponse(
        days=days_value,
        period_start=period_start,
        period_end=period_end,
        behavioral_status=aggregate.status,
        diagnostic=aggregate.diagnostic,
        observed_at=aggregate.observed_at,
        behavioral=behavioral,
        authoritative=authoritative,
        funnel=funnel,
    )


@router.get("/overview", response_model=OverviewResponse)
@limiter.limit("60/minute", key_func=_per_token_rate_key)
async def get_overview(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
):
    """Platform-wide summary metrics for the admin dashboard."""

    now = datetime.now(UTC)
    seven_days_ago = now - timedelta(days=7)
    market_org_ids = _market_member_org_ids_subquery()

    # --- Users ---
    # Count only real market users (BUYER/SUPPLIER). Keeps this metric consistent
    # with every downstream filter, and makes NULL roles explicit non-matches.
    total_users_q = await db.execute(
        select(func.count(User.id)).where(
            User.role.in_(_MARKET_MEMBER_ROLES),
        )
    )
    total_users = total_users_q.scalar() or 0

    active_users_q = await db.execute(
        select(func.count(User.id)).where(
            User.role.in_(_MARKET_MEMBER_ROLES),
            User.last_login >= seven_days_ago,
        )
    )
    active_users_7d = active_users_q.scalar() or 0

    # --- Organizations ---
    total_orgs_q = await db.execute(
        select(func.count(distinct(User.organization_id))).where(
            User.organization_id.is_not(None),
            User.role.in_(_MARKET_MEMBER_ROLES),
        )
    )
    total_organizations = total_orgs_q.scalar() or 0

    # --- Orders ---
    total_orders_q = await db.execute(
        select(func.count(OrderBookOrder.id)).where(
            OrderBookOrder.organization_id.in_(market_org_ids)
        )
    )
    total_orders = total_orders_q.scalar() or 0

    open_orders_q = await db.execute(
        select(func.count(OrderBookOrder.id)).where(
            OrderBookOrder.organization_id.in_(market_org_ids),
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.expires_at.is_(None) | (OrderBookOrder.expires_at > func.now()),
        )
    )
    open_orders = open_orders_q.scalar() or 0

    # --- Trades ---
    total_trades_q = await db.execute(
        select(func.count(Trade.id)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
        )
    )
    total_trades = total_trades_q.scalar() or 0

    confirmed_trades_q = await db.execute(
        select(func.count(Trade.id)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
        )
    )
    confirmed_trades = confirmed_trades_q.scalar() or 0

    # Volume — confirmed/delivered/paid trades only. Keeps this metric
    # consistent with revenue/GMV (both PAID-only) and with confirmed_trades.
    volume_q = await db.execute(
        select(func.coalesce(func.sum(Trade.quantity_mt), 0)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
        )
    )
    total_volume_mt = float(volume_q.scalar() or 0)

    # Revenue = sum of commission_amount_usd from PAID trades
    revenue_q = await db.execute(
        select(func.coalesce(func.sum(Trade.commission_amount_usd), 0)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status == TradeStatus.PAID
        )
    )
    total_revenue_usd = float(revenue_q.scalar() or 0)

    # GMV = sum of final_total_usd from PAID trades
    gmv_q = await db.execute(
        select(func.coalesce(func.sum(Trade.final_total_usd), 0)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status == TradeStatus.PAID
        )
    )
    total_gmv_usd = float(gmv_q.scalar() or 0)

    return OverviewResponse(
        total_users=total_users,
        active_users_7d=active_users_7d,
        total_organizations=total_organizations,
        total_orders=total_orders,
        open_orders=open_orders,
        total_trades=total_trades,
        confirmed_trades=confirmed_trades,
        total_volume_mt=total_volume_mt,
        total_revenue_usd=total_revenue_usd,
        total_gmv_usd=total_gmv_usd,
    )


@router.get("/daily", response_model=list[DailyStat])
@limiter.limit("60/minute", key_func=_per_token_rate_key)
async def get_daily_stats(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    days: int = Query(30, ge=1, le=365),
):
    """Daily trading stats for the last N days."""

    # Bucket days in UTC, not server-local time (Sprint 3 item 6 decision:
    # UTC keeps buckets stable across server moves and matches created_at,
    # which is stored in UTC).
    today = datetime.now(UTC).date()
    start_date = today - timedelta(days=days - 1)
    market_org_ids = _market_member_org_ids_subquery()

    # Daily orders placed
    order_date_col = cast(OrderBookOrder.created_at, Date)
    orders_q = await db.execute(
        select(
            order_date_col.label("day"),
            func.count(OrderBookOrder.id).label("cnt"),
        )
        .where(
            order_date_col >= start_date,
            OrderBookOrder.organization_id.in_(market_org_ids),
        )
        .group_by(order_date_col)
    )
    orders_by_day = {row.day: row.cnt for row in orders_q}

    # Daily trade stats — confirmed/delivered/paid only, matching overview.
    trade_date_col = cast(Trade.created_at, Date)
    trades_q = await db.execute(
        select(
            trade_date_col.label("day"),
            func.count(Trade.id).label("cnt"),
            func.coalesce(func.sum(Trade.quantity_mt), 0).label("vol"),
            func.coalesce(func.sum(Trade.final_total_usd), 0).label("gmv"),
            func.coalesce(func.sum(Trade.commission_amount_usd), 0).label("comm"),
        )
        .where(
            trade_date_col >= start_date,
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status.in_(_CONFIRMED_TRADE_STATUSES),
        )
        .group_by(trade_date_col)
    )
    trades_by_day = {
        row.day: {
            "cnt": row.cnt,
            "vol": float(row.vol),
            "gmv": float(row.gmv),
            "comm": float(row.comm),
        }
        for row in trades_q
    }

    # Build dense array (one entry per day, even if zero activity)
    result: list[DailyStat] = []
    for offset in range(days):
        day = start_date + timedelta(days=offset)
        trade_data = trades_by_day.get(day, {"cnt": 0, "vol": 0.0, "gmv": 0.0, "comm": 0.0})
        result.append(
            DailyStat(
                date=day,
                orders_placed=orders_by_day.get(day, 0),
                trades_executed=trade_data["cnt"],
                volume_mt=trade_data["vol"],
                gmv_usd=trade_data["gmv"],
                commission_usd=trade_data["comm"],
            )
        )

    return result


# ---------------------------------------------------------------------------
# Admin user management
# ---------------------------------------------------------------------------

def _user_to_entry(row) -> AdminUserEntry:
    """Map a user and its organization projection to AdminUserEntry."""
    user, org_name, org_type, org_provenance, org_has_orders = row
    return AdminUserEntry(
        id=user.id,
        email=user.email,
        first_name=user.first_name,
        last_name=user.last_name,
        role=user.role.value if user.role else "",
        status=user.status.value if user.status else "",
        created_at=user.created_at,
        organization_id=user.organization_id,
        org_name=org_name,
        org_type=org_type.value if org_type else None,
        org_provenance=org_provenance.value if org_provenance else None,
        email_verified=bool(user.email_verified),
        must_change_password=bool(user.must_change_password),
        last_login=user.last_login,
        org_has_orders=bool(org_has_orders),
    )


@router.get("/users", response_model=AdminUsersResponse)
@limiter.limit("60/minute", key_func=_per_token_rate_key)
async def list_users(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    status_filter: Optional[str] = Query(None, alias="status"),
    search: Optional[str] = Query(None),
    limit: int = Query(50, ge=1, le=100),
    offset: int = Query(0, ge=0),
):
    """List platform users for admin review. Filterable by status and searchable by name/email."""

    org_has_orders = (
        select(func.count(OrderBookOrder.id) > 0)
        .where(OrderBookOrder.organization_id == User.organization_id)
        .correlate(User)
        .scalar_subquery()
    )
    base = (
        select(
            User,
            Organization.name,
            Organization.type,
            Organization.provenance,
            org_has_orders.label("org_has_orders"),
        )
        .outerjoin(Organization, User.organization_id == Organization.id)
        .where(User.role != UserRole.ADMIN)  # Admins manage non-admin accounts
    )

    if status_filter and status_filter.upper() != "ALL":
        try:
            base = base.where(User.status == UserStatus(status_filter.upper()))
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Invalid status filter: {status_filter}",
            )

    if search:
        term = f"%{search}%"
        base = base.where(
            or_(
                User.email.ilike(term),
                User.first_name.ilike(term),
                User.last_name.ilike(term),
            )
        )

    total_q = await db.execute(select(func.count()).select_from(base.subquery()))
    total = total_q.scalar() or 0

    rows_q = await db.execute(
        base.order_by(User.created_at.desc()).limit(limit).offset(offset)
    )
    items = [_user_to_entry(row) for row in rows_q]

    return AdminUsersResponse(items=items, total=total)


@router.put("/users/{user_id}/reject", response_model=AdminUserEntry)
@limiter.limit("60/minute", key_func=_per_token_rate_key)
async def reject_user(
    request: Request,
    user_id: _uuid.UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
):
    """Reject a pending or approved user. Admins cannot reject other admins."""

    user = (
        await db.execute(select(User).where(User.id == user_id).with_for_update())
    ).scalar_one_or_none()

    if user is None:
        raise HTTPException(status_code=404, detail="User not found")

    if user.role == UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin accounts cannot be rejected via this endpoint",
        )

    if user.status == UserStatus.REJECTED:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="User is already rejected",
        )

    previous_status = user.status
    user.status = UserStatus.REJECTED
    record_status_transition(
        db, user, from_status=previous_status, to_status=UserStatus.REJECTED
    )
    audit_context = request_audit_context(request)
    # A rejected user is fail-closed at execution time: every market mutation
    # and the matching engine re-lock the concrete party and re-check
    # execution_party_is_eligible in-transaction. Tenant-level cleanup of
    # market state is owned by invalidate_organization_market_access on the
    # organization rejection path.
    await record_audit(
        db,
        user_id=current_user.id,
        action=ADMIN_USER_REJECTED,
        resource_type="user",
        resource_id=user.id,
        changes={
            "status": {"from": previous_status.value, "to": UserStatus.REJECTED.value},
            "reason": "admin_analytics_user_rejected",
        },
        **audit_context,
    )
    await db.commit()
    await db.refresh(user)

    organization = (
        await db.execute(
            select(
                Organization.name,
                Organization.type,
                Organization.provenance,
            ).where(
                Organization.id == user.organization_id
            )
        )
    ).one_or_none()
    org_name, org_type, org_provenance = (
        organization if organization is not None else (None, None, None)
    )

    return _user_to_entry((user, org_name, org_type, org_provenance))


# ---------------------------------------------------------------------------
# Product Analytics workspace (plan §2.2): seven lazy tab endpoints
# ---------------------------------------------------------------------------

import asyncio
from decimal import Decimal

from pydantic import ValidationError

from app.models.audit import AuditLog
from app.models.catalog import DeliveryPoint, Product
from app.schemas.product_analytics import (
    AcquisitionKpis,
    AcquisitionResponse,
    ActivationResponse,
    ActivityTrend,
    AggregateCell,
    AnalyticsActivity,
    AnalyticsAudience,
    AnalyticsCoverage,
    AnalyticsDataQuality,
    AnalyticsDiagnostic as SchemaDiagnostic,
    AnalyticsMeta,
    AnalyticsSourceCoverage,
    AnalyticsSourceStatus,
    BackendUnavailablePanel,
    CalculatorFunnel,
    CollectorState,
    ConversionRatioKind,
    CtaMatrixRow,
    DecimalMetricValue,
    EngagementResponse,
    FeatureAdoptionRow,
    FrontendErrorPanel,
    JourneySource,
    JourneyStage,
    LabeledRatio,
    LifecycleStage,
    LifecycleStageKey,
    LoginFailurePanel,
    MarketplaceResponse,
    MetricValue,
    NavigationDestinationRow,
    NavigationLatencyRow,
    NeedsAttentionItem,
    NeedsAttentionRule,
    OverviewResponse as PAOverviewResponse,
    ProductAnalyticsQuery,
    RankedRow,
    RatioValue,
    ReliabilityResponse,
    RetentionKpis,
    RetentionResponse,
    SeriesPoint,
    AuditActivityRow,
)
from app.services.availability_windows import normalize_availability_window
from app.services.behavioral_analytics import BehavioralWindowAggregate
from app.services.product_analytics import ProductAnalyticsService

_PA_RATE_LIMIT = "30/minute"
_ADMIN_ONLY = require_role(UserRole.ADMIN)

_ACQUISITION_EVENTS = ("landing_cta_clicked",)
_ENGAGEMENT_EVENTS = ("platform_navigation", "tutorial_step_completed", "tutorial_step_skipped")
_RELIABILITY_EVENTS = ("login_failed", "frontend_error", "backend_unavailable", "navigation_performance")

_FEATURE_FAMILIES = (
    "platform_navigation", "market_slice_selected", "listing_opened",
    "order_form_opened", "order_form_submitted", "tutorial_started",
    "tutorial_completed", "estimator_opened", "estimator_completed",
)
_LATENCY_BUCKETS = ("lt250", "250_500", "500_1000", "1000_2500", "gte2500")


def product_analytics_query(
    start: Annotated[datetime, Query(description="Half-open UTC period start")],
    end: Annotated[datetime, Query(description="Half-open UTC period end (exclusive)")],
    compare: Annotated[bool, Query()] = True,
    audience: Annotated[AnalyticsAudience, Query()] = AnalyticsAudience.ALL,
    activity: Annotated[AnalyticsActivity, Query()] = AnalyticsActivity.LIVE,
    product_id: Annotated[Optional[_uuid.UUID], Query()] = None,
    delivery_point_id: Annotated[Optional[_uuid.UUID], Query()] = None,
    availability_window: Annotated[Optional[str], Query(max_length=16)] = None,
) -> ProductAnalyticsQuery:
    """Explicit GET-scalar dependency (plan §2.5): each parameter is declared
    with Query(...) and the validated model is constructed here — FastAPI is
    never asked to deserialize a Pydantic body on a GET endpoint."""
    try:
        return ProductAnalyticsQuery(
            start=start,
            end=end,
            compare=compare,
            audience=audience,
            activity=activity,
            product_id=product_id,
            delivery_point_id=delivery_point_id,
            availability_window=availability_window,
        )
    except ValidationError as error:
        first = error.errors()[0] if error.errors() else {}
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail=str(first.get("msg", "invalid product analytics query")),
        )


def _reject_reference_outside_marketplace(query: ProductAnalyticsQuery) -> None:
    if query.activity == AnalyticsActivity.REFERENCE:
        raise HTTPException(
            status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
            detail="REFERENCE activity is accepted only by the marketplace tab",
        )


def _schema_diagnostic(value: str | None) -> SchemaDiagnostic | None:
    if value is None:
        return None
    try:
        return SchemaDiagnostic(value)
    except ValueError:
        return SchemaDiagnostic.UPSTREAM


def _behavioral_coverage(window: BehavioralWindowAggregate | None) -> AnalyticsSourceCoverage:
    if window is None:
        return AnalyticsSourceCoverage(
            status=AnalyticsSourceStatus.UNAVAILABLE,
            diagnostic=SchemaDiagnostic.UPSTREAM,
        )
    status_map = {
        "available": AnalyticsSourceStatus.AVAILABLE,
        "partial": AnalyticsSourceStatus.PARTIAL,
        "unavailable": AnalyticsSourceStatus.UNAVAILABLE,
    }
    return AnalyticsSourceCoverage(
        observed_at=window.observed_at,
        status=status_map.get(window.status, AnalyticsSourceStatus.UNAVAILABLE),
        diagnostic=_schema_diagnostic(window.diagnostic.value if window.diagnostic else None),
    )


def _fact_coverage(
    coverage_start: datetime | None, query: ProductAnalyticsQuery
) -> AnalyticsSourceCoverage:
    if coverage_start is None:
        return AnalyticsSourceCoverage(
            status=AnalyticsSourceStatus.UNAVAILABLE,
            diagnostic=SchemaDiagnostic.INSUFFICIENT_COVERAGE,
        )
    fully_covered = coverage_start <= query.start and (
        query.previous_start is None or coverage_start <= query.previous_start
    )
    return AnalyticsSourceCoverage(
        coverage_start=coverage_start,
        observed_at=datetime.now(UTC),
        status=AnalyticsSourceStatus.AVAILABLE if fully_covered else AnalyticsSourceStatus.PARTIAL,
        diagnostic=None if fully_covered else SchemaDiagnostic.INSUFFICIENT_COVERAGE,
    )


def _meta(
    query: ProductAnalyticsQuery,
    *,
    data_quality: AnalyticsDataQuality,
    behavioral: BehavioralWindowAggregate | None = None,
    behavioral_applicable: bool = True,
    login_coverage: datetime | None = None,
    login_applicable: bool = True,
    status_coverage: datetime | None = None,
    status_applicable: bool = True,
    reference_status: AnalyticsSourceStatus = AnalyticsSourceStatus.NOT_APPLICABLE,
) -> AnalyticsMeta:
    return AnalyticsMeta(
        start=query.start,
        end=query.end,
        previous_start=query.previous_start,
        previous_end=query.previous_end,
        observed_at=datetime.now(UTC),
        coverage=AnalyticsCoverage(
            authoritative=AnalyticsSourceCoverage(
                status=AnalyticsSourceStatus.AVAILABLE, observed_at=datetime.now(UTC)
            ),
            behavioral=(
                _behavioral_coverage(behavioral)
                if behavioral_applicable
                else AnalyticsSourceCoverage.not_applicable()
            ),
            login_history=(
                _fact_coverage(login_coverage, query)
                if login_applicable
                else AnalyticsSourceCoverage.not_applicable()
            ),
            status_history=(
                _fact_coverage(status_coverage, query)
                if status_applicable
                else AnalyticsSourceCoverage.not_applicable()
            ),
            reference=AnalyticsSourceCoverage(status=reference_status),
        ),
        data_quality=data_quality,
    )


async def _behavioral_windows(
    analytics: UmamiAnalyticsService,
    query: ProductAnalyticsQuery,
    events: tuple[str, ...] = (),
) -> tuple[BehavioralWindowAggregate | None, BehavioralWindowAggregate | None]:
    """Fetch current and previous behavioral windows; failures degrade to
    unavailable results and never fail the tab."""
    try:
        current = await analytics.get_window_aggregate(
            query.start, query.end, event_properties=events
        )
    except Exception:
        current = None
    previous = None
    if query.previous_start is not None and query.previous_end is not None:
        try:
            previous = await analytics.get_window_aggregate(
                query.previous_start, query.previous_end, event_properties=events
            )
        except Exception:
            previous = None
    return current, previous


def _behavioral_int(window: BehavioralWindowAggregate | None, field: str) -> int | None:
    if window is None or window.status == "unavailable":
        return None
    return getattr(window, field)


def _event_total(window: BehavioralWindowAggregate | None, event: str) -> int | None:
    if window is None or window.status == "unavailable":
        return None
    return window.event_totals.get(event, 0)


def _behavioral_metric(
    current: BehavioralWindowAggregate | None,
    previous: BehavioralWindowAggregate | None,
    field: str,
) -> MetricValue:
    return MetricValue(
        value=_behavioral_int(current, field), previous=_behavioral_int(previous, field)
    )


def _event_metric(
    current: BehavioralWindowAggregate | None,
    previous: BehavioralWindowAggregate | None,
    event: str,
) -> MetricValue:
    return MetricValue(
        value=_event_total(current, event), previous=_event_total(previous, event)
    )


def _daily_series(
    window: BehavioralWindowAggregate | None, query: ProductAnalyticsQuery
) -> list[SeriesPoint]:
    if window is None or window.status == "unavailable":
        return []
    values: dict[date_type, int] = {}
    for point in window.daily_visitors:
        try:
            values[date_type.fromisoformat(str(point.get("date"))[:10])] = int(point.get("value", 0))
        except (TypeError, ValueError):
            continue
    series: list[SeriesPoint] = []
    cursor = query.start.date()
    while cursor < query.end.date() and len(series) < 366:
        series.append(SeriesPoint(date=cursor, value=values.get(cursor, 0)))
        cursor += timedelta(days=1)
    return series


def _event_daily_series(
    window: BehavioralWindowAggregate | None, event: str, query: ProductAnalyticsQuery
) -> list[SeriesPoint]:
    if window is None or window.status == "unavailable":
        return []
    values: dict[date_type, int] = {}
    for point in window.event_series:
        if point.get("event") != event:
            continue
        try:
            day = date_type.fromisoformat(str(point.get("date"))[:10])
        except (TypeError, ValueError):
            continue
        values[day] = values.get(day, 0) + int(point.get("value", 0))
    series: list[SeriesPoint] = []
    cursor = query.start.date()
    while cursor < query.end.date() and len(series) < 366:
        series.append(SeriesPoint(date=cursor, value=values.get(cursor, 0)))
        cursor += timedelta(days=1)
    return series


def _property_rows(
    window: BehavioralWindowAggregate | None, event: str, property_name: str
) -> list[dict]:
    if window is None:
        return []
    return [
        row
        for row in window.event_properties.get(event, [])
        if row.get("property") == property_name
    ]


def _aggregate_ratio(key: str, numerator: int | None, denominator: int | None) -> LabeledRatio:
    rate = None
    if numerator is not None and denominator and denominator > 0:
        rate = (Decimal(numerator) / Decimal(denominator) * 100).quantize(Decimal("0.01"))
    return LabeledRatio(
        key=key,
        kind=ConversionRatioKind.AGGREGATE_EVENT,
        ratio=RatioValue(numerator=numerator, denominator=denominator, rate_pct=rate),
    )


async def _validate_marketplace_filters(
    db: AsyncSession, query: ProductAnalyticsQuery
) -> None:
    """Canonical catalog validation (§2.1) — only when filters are supplied."""
    if query.availability_window is not None:
        try:
            normalize_availability_window(query.availability_window)
        except ValueError:
            raise HTTPException(
                status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
                detail="availability_window is not a canonical window",
            )
    if query.product_id is not None:
        product = await db.get(Product, query.product_id)
        if product is None or product.is_active is False:
            raise HTTPException(
                status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
                detail="product_id is not an active catalog product",
            )
    if query.delivery_point_id is not None:
        point = await db.get(DeliveryPoint, query.delivery_point_id)
        if point is None or point.is_active is False:
            raise HTTPException(
                status_code=status.HTTP_422_UNPROCESSABLE_ENTITY,
                detail="delivery_point_id is not an active catalog delivery point",
            )


@router.get("/product-analytics/overview", response_model=PAOverviewResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_overview(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    service = ProductAnalyticsService(db)
    authoritative, (behavioral, behavioral_previous) = await asyncio.gather(
        service.overview(query), _behavioral_windows(analytics, query)
    )

    visitors = _behavioral_metric(behavioral, behavioral_previous, "visitors")
    behavioral_status = (
        AnalyticsSourceStatus.AVAILABLE
        if behavioral is not None and behavioral.status != "unavailable"
        else AnalyticsSourceStatus.UNAVAILABLE
    )
    lifecycle = [
        LifecycleStage(
            key=LifecycleStageKey.VISITORS,
            count=visitors.value,
            previous=visitors.previous,
            coverage=behavioral_status,
            detail_tab="acquisition",
        ),
        LifecycleStage(
            key=LifecycleStageKey.REGISTERED,
            count=authoritative.lifecycle.registered.value,
            previous=authoritative.lifecycle.registered.previous,
            coverage=AnalyticsSourceStatus.AVAILABLE,
            detail_tab="activation",
        ),
        LifecycleStage(
            key=LifecycleStageKey.ACTIVE,
            count=authoritative.lifecycle.active.value,
            previous=authoritative.lifecycle.active.previous,
            coverage=_fact_coverage(authoritative.login_coverage_start, query).status,
            detail_tab="engagement",
        ),
        LifecycleStage(
            key=LifecycleStageKey.PARTICIPATING,
            count=authoritative.lifecycle.participating.value,
            previous=authoritative.lifecycle.participating.previous,
            coverage=AnalyticsSourceStatus.AVAILABLE,
            detail_tab="marketplace",
        ),
        LifecycleStage(
            key=LifecycleStageKey.TRADING,
            count=authoritative.lifecycle.trading.value,
            previous=authoritative.lifecycle.trading.previous,
            coverage=AnalyticsSourceStatus.AVAILABLE,
            detail_tab="marketplace",
        ),
        LifecycleStage(
            key=LifecycleStageKey.RETAINED,
            count=authoritative.lifecycle.retained.value,
            previous=authoritative.lifecycle.retained.previous,
            coverage=AnalyticsSourceStatus.AVAILABLE,
            detail_tab="retention",
        ),
    ]

    needs_attention: list[NeedsAttentionItem] = []
    if authoritative.dormant_approved_members > 0:
        needs_attention.append(
            NeedsAttentionItem(
                rule=NeedsAttentionRule.APPROVED_MEMBERS_NEVER_LOGGED_IN,
                count=authoritative.dormant_approved_members,
            )
        )
    signup_started = _event_total(behavioral, "signup_started")
    signup_submitted = _event_total(behavioral, "signup_submitted")
    if signup_started and (signup_submitted or 0) == 0:
        needs_attention.append(
            NeedsAttentionItem(rule=NeedsAttentionRule.SIGNUP_SUBMISSION_DROP, count=signup_started)
        )
    if authoritative.one_sided_live_market:
        needs_attention.append(NeedsAttentionItem(rule=NeedsAttentionRule.ONE_SIDED_LIVE_MARKET))
    login_failures = _event_total(behavioral, "login_failed")
    if login_failures is not None and login_failures >= 5:
        needs_attention.append(
            NeedsAttentionItem(
                rule=NeedsAttentionRule.ELEVATED_LOGIN_FAILURES, count=login_failures
            )
        )
    if behavioral_status != AnalyticsSourceStatus.AVAILABLE:
        needs_attention.append(
            NeedsAttentionItem(rule=NeedsAttentionRule.DEGRADED_ANALYTICS_COLLECTION)
        )

    return PAOverviewResponse(
        meta=_meta(
            query,
            data_quality=authoritative.data_quality,
            behavioral=behavioral,
            login_coverage=authoritative.login_coverage_start,
            status_coverage=authoritative.status_coverage_start,
        ),
        kpis=authoritative.kpis,
        lifecycle=lifecycle,
        activity_trend=ActivityTrend(
            visitors=_daily_series(behavioral, query),
            active_members=authoritative.active_members_series,
            orders=authoritative.orders_series,
            confirmed_trades=authoritative.confirmed_trades_series,
        ),
        marketplace_balance=authoritative.marketplace_balance,
        needs_attention=needs_attention[:10],
    )


@router.get("/product-analytics/acquisition", response_model=AcquisitionResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_acquisition(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    behavioral, behavioral_previous = await _behavioral_windows(
        analytics, query, _ACQUISITION_EVENTS
    )

    def duration(window: BehavioralWindowAggregate | None) -> Decimal | None:
        if window is None or window.status == "unavailable" or not window.visits:
            return None
        return (Decimal(window.total_time_seconds) / Decimal(window.visits)).quantize(
            Decimal("0.1")
        )

    referrer_total = (
        sum(row["value"] for row in behavioral.top_referrers) if behavioral else 0
    )

    def share(count: int) -> Decimal | None:
        if not referrer_total:
            return None
        return (Decimal(count) / Decimal(referrer_total) * 100).quantize(Decimal("0.01"))

    referrers = []
    entry_pages = []
    if behavioral is not None and behavioral.status != "unavailable":
        for row in behavioral.top_referrers[:20]:
            name = str(row.get("name", ""))
            # An empty referrer is Direct / unknown — distinct from no data.
            key = "direct" if name == "" else name
            referrers.append(
                RankedRow(key=key, label=key, count=int(row.get("value", 0)), share_pct=share(int(row.get("value", 0))))
            )
        entry_total = sum(int(row.get("value", 0)) for row in behavioral.top_entries) or None
        for row in behavioral.top_entries[:20]:
            count = int(row.get("value", 0))
            entry_pages.append(
                RankedRow(
                    key=str(row.get("name", "/")),
                    label=str(row.get("name", "/")),
                    count=count,
                    share_pct=(
                        (Decimal(count) / Decimal(entry_total) * 100).quantize(Decimal("0.01"))
                        if entry_total
                        else None
                    ),
                )
            )

    # The verified property API returns per-property value totals; a CTA ×
    # placement cross-tab is not available, so each dimension reports its own
    # rows and this is labelled clicks, never conversion.
    cta_rows: list[CtaMatrixRow] = []
    cta_total = _event_total(behavioral, "landing_cta_clicked")
    for row in _property_rows(behavioral, "landing_cta_clicked", "cta")[:10]:
        cta_rows.append(
            CtaMatrixRow(
                cta=str(row["value"]),
                placement="all",
                clicks=int(row["total"]),
                share_pct=(
                    (Decimal(row["total"]) / Decimal(cta_total) * 100).quantize(Decimal("0.01"))
                    if cta_total
                    else None
                ),
            )
        )
    for row in _property_rows(behavioral, "landing_cta_clicked", "placement")[:10]:
        cta_rows.append(
            CtaMatrixRow(cta="all", placement=str(row["value"]), clicks=int(row["total"]))
        )

    languages = [
        RankedRow(key=str(row["value"]), label=str(row["value"]), count=int(row["total"]))
        for row in _property_rows(behavioral, "landing_cta_clicked", "language")[:5]
    ]

    return AcquisitionResponse(
        meta=_meta(
            query,
            data_quality=AnalyticsDataQuality(),
            behavioral=behavioral,
            login_applicable=False,
            status_applicable=False,
        ),
        kpis=AcquisitionKpis(
            visitors=_behavioral_metric(behavioral, behavioral_previous, "visitors"),
            visits=_behavioral_metric(behavioral, behavioral_previous, "visits"),
            pageviews=_behavioral_metric(behavioral, behavioral_previous, "pageviews"),
            average_session_duration_seconds=DecimalMetricValue(
                value=duration(behavioral), previous=duration(behavioral_previous)
            ),
            cta_clicks=_event_metric(behavioral, behavioral_previous, "landing_cta_clicked"),
        ),
        visitors_trend=_daily_series(behavioral, query),
        previous_visitors_trend=[],
        visits_trend=[],
        referrers=referrers,
        entry_pages=entry_pages,
        cta_matrix=cta_rows[:20],
        languages=languages,
        calculator=CalculatorFunnel(
            starts=_event_metric(behavioral, behavioral_previous, "energy_calculator_started"),
            completions=_event_metric(
                behavioral, behavioral_previous, "energy_calculator_completed"
            ),
        ),
    )


@router.get("/product-analytics/activation", response_model=ActivationResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_activation(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    service = ProductAnalyticsService(db)
    authoritative, (behavioral, _previous) = await asyncio.gather(
        service.activation(query), _behavioral_windows(analytics, query)
    )

    def behavioral_stage(key: str) -> JourneyStage:
        total = _event_total(behavioral, key)
        return JourneyStage(
            key=key,
            source=JourneySource.BEHAVIORAL,
            total=AggregateCell(key=key, count=total),
        )

    journey = [
        behavioral_stage("signup_started"),
        behavioral_stage("signup_role_selected"),
        behavioral_stage("signup_submitted"),
        behavioral_stage("signup_organization_required"),
        behavioral_stage("signup_organization_submitted"),
        JourneyStage(
            key="registered",
            source=JourneySource.AUTHORITATIVE,
            total=AggregateCell(key="registered", count=authoritative.registered_total.value),
            buyer=authoritative.registered_buyer,
            supplier=authoritative.registered_supplier,
        ),
        JourneyStage(
            key="approved",
            source=JourneySource.AUTHORITATIVE,
            total=authoritative.approved_members,
        ),
        JourneyStage(
            key="first_login",
            source=JourneySource.AUTHORITATIVE,
            total=authoritative.first_login_members,
        ),
        JourneyStage(
            key="first_live_order",
            source=JourneySource.AUTHORITATIVE,
            total=authoritative.first_live_order_organizations,
        ),
    ]

    # Aggregate event ratios stay behavioral-only; user/organization cohorts
    # stay database-only — historical DB registrations are never divided by
    # events collected only after behavioral coverage began (§1.6).
    ratios = [
        _aggregate_ratio(
            "signup_started_to_submitted",
            _event_total(behavioral, "signup_submitted"),
            _event_total(behavioral, "signup_started"),
        ),
    ]

    return ActivationResponse(
        meta=_meta(
            query,
            data_quality=authoritative.data_quality,
            behavioral=behavioral,
            login_coverage=authoritative.login_coverage_start,
            status_coverage=authoritative.status_coverage_start,
        ),
        journey=journey,
        time_to_first_login=authoritative.time_to_first_login,
        time_to_first_live_order=authoritative.time_to_first_live_order,
        drop_off=authoritative.drop_off,
        ratios=ratios,
    )


@router.get("/product-analytics/engagement", response_model=EngagementResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_engagement(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    service = ProductAnalyticsService(db)
    authoritative, (behavioral, _previous) = await asyncio.gather(
        service.engagement(query), _behavioral_windows(analytics, query, _ENGAGEMENT_EVENTS)
    )

    adoption = [
        FeatureAdoptionRow(family=family, events=_event_total(behavioral, family))
        for family in _FEATURE_FAMILIES
    ]
    adoption.sort(key=lambda row: row.events or 0, reverse=True)

    workflow_ratios = [
        _aggregate_ratio(
            "listing_to_order_form",
            _event_total(behavioral, "order_form_opened"),
            _event_total(behavioral, "listing_opened"),
        ),
        _aggregate_ratio(
            "order_form_to_submit",
            _event_total(behavioral, "order_form_submitted"),
            _event_total(behavioral, "order_form_opened"),
        ),
        _aggregate_ratio(
            "tutorial_start_to_complete",
            _event_total(behavioral, "tutorial_completed"),
            _event_total(behavioral, "tutorial_started"),
        ),
        _aggregate_ratio(
            "estimator_open_to_complete",
            _event_total(behavioral, "estimator_completed"),
            _event_total(behavioral, "estimator_opened"),
        ),
    ]

    destinations = [
        NavigationDestinationRow(
            destination=str(row["value"]),  # validated by the response model
            total=AggregateCell(key=str(row["value"]), count=int(row["total"])),
        )
        for row in _property_rows(behavioral, "platform_navigation", "destination")[:12]
        if str(row["value"]) in {
            "home", "map", "marketplace", "curve", "watchlist", "analytics",
            "trades", "quotes", "compliance", "training", "settings", "admin",
        }
    ]

    steps: dict[str, dict[str, int]] = {}
    for row in _property_rows(behavioral, "tutorial_step_completed", "step"):
        steps.setdefault(str(row["value"]), {})["completed"] = int(row["total"])
    for row in _property_rows(behavioral, "tutorial_step_skipped", "step"):
        steps.setdefault(str(row["value"]), {})["skipped"] = int(row["total"])
    from app.schemas.product_analytics import TutorialStepRow

    tutorial_steps = [
        TutorialStepRow(
            step=step,
            completed=AggregateCell(key="completed", count=values.get("completed", 0)),
            skipped=AggregateCell(key="skipped", count=values.get("skipped", 0)),
        )
        for step, values in sorted(steps.items())[:20]
    ]

    return EngagementResponse(
        meta=_meta(
            query,
            data_quality=authoritative.data_quality,
            behavioral=behavioral,
            login_coverage=authoritative.login_coverage_start,
            status_applicable=False,
        ),
        kpis=authoritative.kpis,
        active_members_trend=authoritative.active_members_trend,
        feature_adoption=adoption,
        workflow_ratios=workflow_ratios,
        navigation_destinations=destinations,
        tutorial_steps=tutorial_steps,
    )


@router.get("/product-analytics/marketplace", response_model=MarketplaceResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_marketplace(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    await _validate_marketplace_filters(db, query)
    service = ProductAnalyticsService(db)
    result = await service.marketplace(query)

    reference_status = (
        AnalyticsSourceStatus.AVAILABLE
        if result.reference is not None
        else AnalyticsSourceStatus.NOT_APPLICABLE
    )
    return MarketplaceResponse(
        meta=_meta(
            query,
            data_quality=result.data_quality,
            behavioral_applicable=False,
            login_applicable=False,
            status_applicable=False,
            reference_status=reference_status,
        ),
        live=result.live,
        demo=result.demo,
        unknown=result.unknown,
        reference=result.reference,
        commercial=result.commercial,
    )


@router.get("/product-analytics/retention", response_model=RetentionResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_retention(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    service = ProductAnalyticsService(db)
    result = await service.retention(query)

    return RetentionResponse(
        meta=_meta(
            query,
            data_quality=result.data_quality,
            behavioral_applicable=False,
            login_coverage=result.login_coverage_start,
            status_applicable=False,
        ),
        kpis=RetentionKpis(
            returning_members=result.returning_members,
            retained_organizations=result.retained_organizations,
            reactivated_organizations=result.reactivated_organizations,
            dormant_approved_members=result.dormant_approved_members,
        ),
        member_cohorts=result.member_cohorts[:54],
        organization_cohorts=result.organization_cohorts[:54],
        repeat_participation=result.repeat_participation,
    )


@router.get("/product-analytics/reliability", response_model=ReliabilityResponse)
@limiter.limit(_PA_RATE_LIMIT, key_func=_per_token_rate_key)
async def product_analytics_reliability(
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(_ADMIN_ONLY)],
    analytics: Annotated[UmamiAnalyticsService, Depends(get_analytics_service)],
    query: Annotated[ProductAnalyticsQuery, Depends(product_analytics_query)],
):
    _reject_reference_outside_marketplace(query)
    behavioral, behavioral_previous = await _behavioral_windows(
        analytics, query, _RELIABILITY_EVENTS
    )

    def cells(event: str, property_name: str, limit: int = 8) -> list[AggregateCell]:
        return [
            AggregateCell(key=str(row["value"]), count=int(row["total"]))
            for row in _property_rows(behavioral, event, property_name)[:limit]
        ]

    latency_cells = {
        str(row["value"]): int(row["total"])
        for row in _property_rows(behavioral, "navigation_performance", "latency_bucket")
    }
    navigation_latency = []
    if latency_cells:
        # The per-destination cross-tab is unavailable from the verified
        # property API; "all" carries the cross-destination distribution.
        navigation_latency.append(
            NavigationLatencyRow(
                destination="all",
                buckets=[
                    AggregateCell(key=bucket, count=latency_cells.get(bucket, 0))
                    for bucket in _LATENCY_BUCKETS
                ],
            )
        )

    audit_rows = (
        await db.execute(
            select(AuditLog.timestamp, AuditLog.action, AuditLog.resource_type, User.role)
            .outerjoin(User, User.id == AuditLog.user_id)
            .order_by(AuditLog.timestamp.desc())
            .limit(20)
        )
    ).all()

    return ReliabilityResponse(
        meta=_meta(
            query,
            data_quality=AnalyticsDataQuality(),
            behavioral=behavioral,
            login_applicable=False,
            status_applicable=False,
        ),
        collector=CollectorState(
            status=_behavioral_coverage(behavioral).status,
            diagnostic=_behavioral_coverage(behavioral).diagnostic,
            last_observation_at=behavioral.observed_at if behavioral else None,
        ),
        login_failures=LoginFailurePanel(
            total=_event_metric(behavioral, behavioral_previous, "login_failed"),
            categories=cells("login_failed", "reason_category"),
            trend=_event_daily_series(behavioral, "login_failed", query),
        ),
        frontend_errors=FrontendErrorPanel(
            total=_event_metric(behavioral, behavioral_previous, "frontend_error"),
            by_route_family=cells("frontend_error", "route_family", 4),
            by_category=cells("frontend_error", "category", 4),
        ),
        backend_unavailable=BackendUnavailablePanel(
            total=_event_metric(behavioral, behavioral_previous, "backend_unavailable"),
            by_route_family=cells("backend_unavailable", "route_family", 4),
        ),
        navigation_latency=navigation_latency,
        # Product reliability telemetry only — no IP addresses, no actors.
        audit_activity=[
            AuditActivityRow(
                occurred_at=row.timestamp,
                action=row.action,
                resource_type=row.resource_type,
                actor_role=row.role.value if row.role else None,
            )
            for row in audit_rows
        ],
    )
