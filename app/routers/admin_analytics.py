"""Platform-wide analytics endpoints for the admin dashboard."""
import hashlib
import uuid as _uuid
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
    org_name: Optional[str]
    org_type: Optional[str]

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
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED])
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
    """Map a (User, org_name, org_type) row to AdminUserEntry."""
    user, org_name, org_type = row
    return AdminUserEntry(
        id=user.id,
        email=user.email,
        first_name=user.first_name,
        last_name=user.last_name,
        role=user.role.value if user.role else "",
        status=user.status.value if user.status else "",
        created_at=user.created_at,
        org_name=org_name,
        org_type=org_type.value if org_type else None,
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

    base = (
        select(User, Organization.name, Organization.type)
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

    result = await db.execute(
        select(User, Organization.name, Organization.type)
        .outerjoin(Organization, User.organization_id == Organization.id)
        .where(User.id == user_id)
    )
    row = result.one_or_none()

    if row is None:
        raise HTTPException(status_code=404, detail="User not found")

    user, org_name, org_type = row

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
    await record_audit(
        db,
        user_id=current_user.id,
        action=ADMIN_USER_REJECTED,
        resource_type="user",
        resource_id=user.id,
        changes={"status": {"from": previous_status.value, "to": UserStatus.REJECTED.value}},
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(user)

    return _user_to_entry((user, org_name, org_type))
