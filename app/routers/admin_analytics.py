"""Platform-wide analytics endpoints for the admin dashboard."""
from datetime import datetime, timedelta, UTC, date
from typing import Annotated

from fastapi import APIRouter, Depends, Query
from pydantic import BaseModel
from sqlalchemy import select, func, cast, Date, distinct
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.models.orderbook import (
    OrderBookOrder, OrderBookStatus, Trade, TradeStatus,
)
from app.models.user import User, UserRole


# ---------------------------------------------------------------------------
# Response schemas
# ---------------------------------------------------------------------------

class OverviewResponse(BaseModel):
    total_users: int
    active_users_7d: int
    total_organizations: int
    total_orders: int
    open_orders: int
    total_trades: int
    confirmed_trades: int
    total_volume_mt: float
    total_revenue_usd: float
    total_gmv_usd: float


class DailyStat(BaseModel):
    date: date
    orders_placed: int
    trades_executed: int
    volume_mt: float
    gmv_usd: float
    commission_usd: float


# ---------------------------------------------------------------------------
# Router
# ---------------------------------------------------------------------------

router = APIRouter(prefix="/admin/analytics", tags=["admin-analytics"])


_MARKET_MEMBER_ROLES = [UserRole.BUYER, UserRole.SUPPLIER]


def _market_member_org_ids_subquery():
    """
    Organizations that have at least one real (BUYER/SUPPLIER) user.

    Excludes seeded/demo liquidity orgs that have no real member accounts,
    as well as admin-only orgs. Uses an explicit role whitelist so that
    NULL roles and any future non-market roles are also excluded.

    Note on trade filtering: callers use this subquery with AND semantics
    (both buyer AND seller must be market members). Trades where a real
    user transacts against a seeded liquidity org will therefore not be
    counted. If the pilot requires surfacing real-vs-seed trades,
    switch the trade filters to OR (at least one side is a market org).
    """
    return (
        select(distinct(User.organization_id))
        .where(
            User.organization_id.is_not(None),
            User.role.in_(_MARKET_MEMBER_ROLES),
        )
    )


@router.get("/overview", response_model=OverviewResponse)
async def get_overview(
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

    confirmed_statuses = [TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID]
    confirmed_trades_q = await db.execute(
        select(func.count(Trade.id)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
            Trade.status.in_(confirmed_statuses),
        )
    )
    confirmed_trades = confirmed_trades_q.scalar() or 0

    # Total volume (all trades)
    volume_q = await db.execute(
        select(func.coalesce(func.sum(Trade.quantity_mt), 0)).where(
            Trade.buyer_id.in_(market_org_ids),
            Trade.seller_id.in_(market_org_ids),
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
async def get_daily_stats(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    days: int = Query(30, ge=1, le=365),
):
    """Daily trading stats for the last N days."""

    today = date.today()
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

    # Daily trade stats
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
