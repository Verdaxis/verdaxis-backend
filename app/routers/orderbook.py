from fastapi import APIRouter, Depends, HTTPException, status, Query
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func
from sqlalchemy.orm import selectinload
from typing import Optional
from uuid import UUID

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User, UserRole
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.schemas.orderbook import (
    OrderCreate,
    OrderUpdate,
    OrderResponse,
    OrderMyResponse,
    AggregatedOrderbookResponse,
    OrderResponseWithCI,
)
from app.services.ci_pricing import calculate_ci_adjusted_price

router = APIRouter(prefix="/orderbook", tags=["orderbook"])


# ============== Static routes (must come before parametric /{order_id}) ==============


@router.get("/bids", response_model=list[OrderResponse])
async def list_bids(
    region: Optional[str] = Query(None, description="Filter by region"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open BID orders.
    """
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.side == OrderSide.BID,
        )
    )

    if region:
        query = query.where(OrderBookOrder.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if availability_window:
        query = query.where(OrderBookOrder.availability_window == availability_window)

    query = query.order_by(OrderBookOrder.created_at.desc())
    result = await db.execute(query)
    orders = result.scalars().all()
    return orders


@router.get("/asks", response_model=list[OrderResponse])
async def list_asks(
    region: Optional[str] = Query(None, description="Filter by region"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open ASK orders.
    """
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            OrderBookOrder.side == OrderSide.ASK,
        )
    )

    if region:
        query = query.where(OrderBookOrder.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if availability_window:
        query = query.where(OrderBookOrder.availability_window == availability_window)

    query = query.order_by(OrderBookOrder.created_at.desc())
    result = await db.execute(query)
    orders = result.scalars().all()
    return orders


@router.get("/with-ci", response_model=list[OrderResponseWithCI])
async def list_orders_with_ci(
    region: Optional[str] = Query(None),
    fuel_type: Optional[str] = Query(None),
    side: Optional[OrderSide] = Query(None),
    db: AsyncSession = Depends(get_db),
):
    """
    List open orders enriched with CI-adjusted pricing.
    Orders that have carbon_intensity and energy_density populated
    will include the ci_adjusted_price object.
    """
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]))
    )
    if region:
        query = query.where(OrderBookOrder.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if side:
        query = query.where(OrderBookOrder.side == side)
    query = query.order_by(OrderBookOrder.created_at.desc())

    result = await db.execute(query)
    orders = result.scalars().all()

    enriched = []
    for order in orders:
        ci_price = None
        if order.carbon_intensity_gco2_mj and order.energy_density_mj_kg:
            ci_price = calculate_ci_adjusted_price(
                base_price_per_mt=order.price_per_mt_usd,
                carbon_intensity_gco2_mj=order.carbon_intensity_gco2_mj,
                energy_density_mj_kg=order.energy_density_mj_kg,
            )
        resp = OrderResponseWithCI.model_validate(order)
        resp.ci_adjusted_price = ci_price
        enriched.append(resp)

    return enriched


@router.get("/my", response_model=list[OrderMyResponse])
async def list_my_orders(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    List current user's own orders (both bids and asks).
    """
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    query = (
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.bid_trades),
            selectinload(OrderBookOrder.ask_trades),
        )
        .where(OrderBookOrder.organization_id == current_user.organization_id)
        .order_by(OrderBookOrder.created_at.desc())
    )

    result = await db.execute(query)
    orders = result.scalars().all()

    result_list = []
    for order in orders:
        item = OrderMyResponse.model_validate(order)
        if order.side == OrderSide.BID:
            item.trade_count = len(order.bid_trades)
        else:
            item.trade_count = len(order.ask_trades)
        result_list.append(item)

    return result_list


@router.get("/aggregated", response_model=list[AggregatedOrderbookResponse])
async def list_aggregated_orderbook(
    db: AsyncSession = Depends(get_db),
):
    """
    Market data aggregated by region, fuel type, and side.
    """
    query = (
        select(
            OrderBookOrder.region,
            OrderBookOrder.fuel_type,
            OrderBookOrder.side,
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_quantity"),
            func.count(OrderBookOrder.id).label("order_count"),
        )
        .where(OrderBookOrder.status == OrderBookStatus.OPEN)
        .group_by(OrderBookOrder.region, OrderBookOrder.fuel_type, OrderBookOrder.side)
        .order_by(OrderBookOrder.region, OrderBookOrder.fuel_type, OrderBookOrder.side)
    )

    result = await db.execute(query)
    rows = result.all()

    aggregated_data = []
    for row in rows:
        aggregated_data.append(
            AggregatedOrderbookResponse(
                region=row.region,
                fuel_type=row.fuel_type,
                side=row.side,
                min_price=row.min_price,
                max_price=row.max_price,
                total_quantity=row.total_quantity,
                order_count=row.order_count,
            )
        )

    return aggregated_data


@router.get("/regions", response_model=list[str])
async def list_regions(db: AsyncSession = Depends(get_db)):
    """
    Get distinct regions from open orders.
    """
    query = (
        select(OrderBookOrder.region)
        .where(OrderBookOrder.status == OrderBookStatus.OPEN)
        .distinct()
    )
    result = await db.execute(query)
    regions = result.scalars().all()
    return list(regions)


@router.get("/fuel-types", response_model=list[str])
async def list_fuel_types(db: AsyncSession = Depends(get_db)):
    """
    Get distinct fuel types from open orders.
    """
    query = (
        select(OrderBookOrder.fuel_type)
        .where(OrderBookOrder.status == OrderBookStatus.OPEN)
        .distinct()
    )
    result = await db.execute(query)
    fuel_types = result.scalars().all()
    return list(fuel_types)


# ============== List all + CRUD (parametric routes last) ==============


@router.get("", response_model=list[OrderResponse])
async def list_orders(
    region: Optional[str] = Query(None, description="Filter by region"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    side: Optional[OrderSide] = Query(None, description="Filter by side (BID or ASK)"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open and partially filled orders (bids + asks).
    """
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED])
        )
    )

    if region:
        query = query.where(OrderBookOrder.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.where(OrderBookOrder.fuel_type.ilike(f"%{fuel_type}%"))
    if side:
        query = query.where(OrderBookOrder.side == side)
    if availability_window:
        query = query.where(OrderBookOrder.availability_window == availability_window)

    query = query.order_by(OrderBookOrder.created_at.desc())
    result = await db.execute(query)
    orders = result.scalars().all()
    return orders


@router.post("", response_model=OrderResponse, status_code=status.HTTP_201_CREATED)
async def create_order(
    order_data: OrderCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Place a new order. BID requires BUYER role, ASK requires SUPPLIER role.
    """
    if order_data.side == OrderSide.BID and current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can place BID orders",
        )

    if order_data.side == OrderSide.ASK and current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can place ASK orders",
        )

    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    new_order = OrderBookOrder(
        organization_id=current_user.organization_id,
        side=order_data.side,
        fuel_type=order_data.fuel_type,
        fuel_grade=order_data.fuel_grade,
        region=order_data.region,
        port_id=order_data.port_id,
        vessel_id=order_data.vessel_id,
        quantity_mt=order_data.quantity_mt,
        remaining_quantity_mt=order_data.quantity_mt,
        price_per_mt_usd=order_data.price_per_mt_usd,
        availability_window=order_data.availability_window,
        delivery_window_start=order_data.delivery_window_start,
        delivery_window_end=order_data.delivery_window_end,
        expires_at=order_data.expires_at,
    )

    if order_data.side == OrderSide.ASK:
        new_order.certifications = order_data.certifications

    db.add(new_order)
    await db.commit()
    await db.refresh(new_order)

    # Re-fetch with eager loading so tier_label computed property works
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == new_order.id)
    )
    new_order = result.scalars().first()

    return new_order


@router.put("/{order_id}", response_model=OrderResponse)
async def update_order(
    order_id: UUID,
    update_data: OrderUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Update an own order. Only allowed if status is OPEN or PARTIALLY_FILLED.
    """
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == order_id)
    )
    order = result.scalars().first()

    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found",
        )

    if order.organization_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only update your own orders",
        )

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only update orders with OPEN or PARTIALLY_FILLED status",
        )

    update_dict = update_data.model_dump(exclude_unset=True)

    # If quantity_mt changes, recalculate remaining_quantity_mt proportionally
    if "quantity_mt" in update_dict:
        old_quantity = order.quantity_mt
        old_remaining = order.remaining_quantity_mt
        new_quantity = update_dict["quantity_mt"]
        filled = old_quantity - old_remaining
        new_remaining = new_quantity - filled
        if new_remaining < 0:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="New quantity cannot be less than already filled amount",
            )
        update_dict["remaining_quantity_mt"] = new_remaining

    for field, value in update_dict.items():
        setattr(order, field, value)

    await db.commit()
    await db.refresh(order)

    # Re-fetch with eager loading for tier_label
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == order.id)
    )
    order = result.scalars().first()

    return order


@router.delete("/{order_id}", status_code=status.HTTP_204_NO_CONTENT)
async def cancel_order(
    order_id: UUID,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Cancel an own order (soft cancel by setting status to CANCELLED).
    """
    result = await db.execute(
        select(OrderBookOrder).where(OrderBookOrder.id == order_id)
    )
    order = result.scalars().first()

    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found",
        )

    if order.organization_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only cancel your own orders",
        )

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only cancel orders with OPEN or PARTIALLY_FILLED status",
        )

    order.status = OrderBookStatus.CANCELLED
    await db.commit()

    return None
