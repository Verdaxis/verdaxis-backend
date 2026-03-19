"""
Tiered Data Products API — S7-005.

GET /api/data/reference-prices

Access tiers (based on token type):
  - Public (no auth):         yesterday's daily VWAP only
  - Free (user/free OAuth2):  hourly VWAP (last 24h)
  - Paid (OAuth2 paid tier):  real-time VWAP + additional fields
"""
from datetime import datetime, date, timedelta, UTC
from decimal import Decimal
from typing import Optional, Any

from fastapi import APIRouter, Depends, Query, Request
from fastapi.security import OAuth2PasswordBearer
from sqlalchemy import select, func, cast, Date
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.orderbook import Trade, TradeStatus, OrderBookOrder
from app.rate_limit import limiter

router = APIRouter(prefix="/data", tags=["data-products"])

_oauth2_optional = OAuth2PasswordBearer(tokenUrl="/api/auth/login", auto_error=False)


def _decode_token_safe(token: Optional[str]) -> Optional[dict]:
    """Decode a JWT without raising — returns None on any failure."""
    if not token:
        return None
    try:
        from app.core.security import decode_token
        return decode_token(token)
    except Exception:
        return None


def _resolve_tier(token: Optional[str]) -> str:
    """
    Resolve the caller's data tier from the bearer token.

    - No token or invalid token: "public"
    - Valid user token (no token_kind): "free"
    - OAuth2 client token with scopes containing 'read:market' and
      rate_limit_tier == "paid": "paid"
    - OAuth2 client token with 'read:market' and any other tier: "free"
    - OAuth2 client token without 'read:market' scope: treated as "public"
      (scope enforcement for OAuth2 clients accessing data products)
    """
    payload = _decode_token_safe(token)
    if payload is None:
        return "public"
    token_kind = payload.get("token_kind")
    if token_kind == "oauth2_client":
        # OAuth2 clients must have read:market scope to access data products
        scopes: list[str] = payload.get("scopes", [])
        if "read:market" not in scopes:
            return "public"
        return payload.get("rate_limit_tier", "free")
    return "free"


async def _compute_vwap(
    db: AsyncSession,
    *,
    date_filter: Optional[date],
    hours_filter: Optional[int],
    fuel_type: Optional[str],
    region: Optional[str],
) -> list[dict]:
    """
    Shared VWAP computation. Use exactly one of date_filter (daily) or hours_filter (hourly/realtime).
    Returns list of dicts with keys: fuel_type, region, vwap_usd, total_volume_mt, trade_count, period.
    """
    valid_statuses = [TradeStatus.CONFIRMED, TradeStatus.DELIVERED, TradeStatus.PAID]

    if hours_filter is not None:
        cutoff = datetime.now(UTC) - timedelta(hours=hours_filter)
        time_label = func.strftime("%Y-%m-%dT%H:00", Trade.created_at).label("period")
        time_group = func.strftime("%Y-%m-%dT%H:00", Trade.created_at)
        stmt = (
            select(
                OrderBookOrder.fuel_type.label("fuel_type"),
                OrderBookOrder.region.label("region"),
                time_label,
                func.sum(Trade.price_per_mt_usd * Trade.quantity_mt).label("weighted_sum"),
                func.sum(Trade.quantity_mt).label("total_volume"),
                func.count(Trade.id).label("trade_count"),
            )
            .join(OrderBookOrder, Trade.ask_order_id == OrderBookOrder.id)
            .where(Trade.status.in_(valid_statuses), Trade.created_at >= cutoff)
        )
        group_by_period = time_group
    else:
        # Daily — filter to exactly date_filter
        target_date = date_filter or (datetime.now(UTC).date() - timedelta(days=1))
        stmt = (
            select(
                OrderBookOrder.fuel_type.label("fuel_type"),
                OrderBookOrder.region.label("region"),
                cast(Trade.created_at, Date).label("period"),
                func.sum(Trade.price_per_mt_usd * Trade.quantity_mt).label("weighted_sum"),
                func.sum(Trade.quantity_mt).label("total_volume"),
                func.count(Trade.id).label("trade_count"),
            )
            .join(OrderBookOrder, Trade.ask_order_id == OrderBookOrder.id)
            .where(
                Trade.status.in_(valid_statuses),
                cast(Trade.created_at, Date) == target_date,
            )
        )
        group_by_period = cast(Trade.created_at, Date)

    if fuel_type:
        stmt = stmt.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if region:
        stmt = stmt.where(OrderBookOrder.region.ilike(f"%{region}%"))

    stmt = stmt.group_by(
        OrderBookOrder.fuel_type,
        OrderBookOrder.region,
        group_by_period,
    ).order_by(group_by_period.desc())

    result = await db.execute(stmt)
    rows = result.all()

    items = []
    for row in rows:
        total_vol = row.total_volume or Decimal("0")
        weighted = row.weighted_sum or Decimal("0")
        vwap = Decimal(str(round(weighted / total_vol, 4))) if total_vol > 0 else Decimal("0")
        items.append({
            "fuel_type": row.fuel_type,
            "region": row.region,
            "vwap_usd": str(vwap),
            "total_volume_mt": str(total_vol),
            "trade_count": row.trade_count or 0,
            "period": str(row.period),
        })
    return items


@router.get("/reference-prices")
@limiter.limit("60/minute")
async def get_tiered_reference_prices(
    request: Request,
    fuel_type: Optional[str] = Query(None),
    region: Optional[str] = Query(None),
    token: Optional[str] = Depends(_oauth2_optional),
    db: AsyncSession = Depends(get_db),
) -> dict[str, Any]:
    """
    Tiered reference-price endpoint.

    OAuth2 client tokens must carry the 'read:market' scope; without it they
    are treated as unauthenticated (public tier). User session tokens always
    get the free tier. No auth is required for public (daily VWAP) access.

    - Public  → yesterday's daily VWAP only
    - Free    → hourly VWAP for the last 24 hours
    - Paid    → real-time VWAP (last 1 hour) + additional metadata fields
    """
    tier = _resolve_tier(token)

    if tier == "paid":
        items = await _compute_vwap(db, date_filter=None, hours_filter=1, fuel_type=fuel_type, region=region)
        return {
            "tier": tier,
            "resolution": "realtime",
            "generated_at": datetime.now(UTC).isoformat(),
            "prices": items,
            "metadata": {
                "window_hours": 1,
                "latency_seconds": 0,
                "note": "Real-time VWAP from confirmed trades in the last 60 minutes.",
            },
        }
    elif tier == "free":
        items = await _compute_vwap(db, date_filter=None, hours_filter=24, fuel_type=fuel_type, region=region)
        return {
            "tier": tier,
            "resolution": "hourly",
            "generated_at": datetime.now(UTC).isoformat(),
            "prices": items,
        }
    else:
        # public
        yesterday = datetime.now(UTC).date() - timedelta(days=1)
        items = await _compute_vwap(db, date_filter=yesterday, hours_filter=None, fuel_type=fuel_type, region=region)
        return {
            "tier": tier,
            "resolution": "daily",
            "generated_at": datetime.now(UTC).isoformat(),
            "prices": items,
            "note": "Authenticate to access hourly or real-time data.",
        }
