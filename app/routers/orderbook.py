from fastapi import APIRouter, Depends, HTTPException, status, Query
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func, or_, and_
from sqlalchemy.orm import selectinload, joinedload
from typing import Optional
from decimal import Decimal
from uuid import UUID

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User, UserRole
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus
from app.models.catalog import Product, DeliveryPoint
from app.schemas.orderbook import (
    OrderCreate,
    OrderUpdate,
    OrderResponse,
    OrderMyResponse,
    SupplierListingTemplateResponse,
    AggregatedOrderbookResponse,
    OrderResponseWithCI,
)
from app.schemas.pagination import PaginatedResponse
from app.services.ci_pricing import calculate_ci_adjusted_price
from app.services.event_bus import event_bus
from app.services.availability_windows import normalize_availability_window
from pydantic import BaseModel
from app.services.benchmarks import compute_premium_discount, get_benchmark_quote

router = APIRouter(prefix="/orderbook", tags=["orderbook"])

SUPPLIER_METADATA_FIELDS = (
    "certification_declared",
    "certification_scheme",
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "carbon_intensity_method",
    "feedstock",
    "origin",
    "off_spec",
    "off_spec_notes",
)

APPROVED_MARKETPLACE_FUEL_TYPES = ("Methanol", "Ethanol")


def _ensure_join(joins: list[tuple[object, object]], target: object, condition: object) -> None:
    if not any(existing_target == target for existing_target, _ in joins):
        joins.append((target, condition))


def _apply_public_marketplace_scope(filters: list[object], joins: list[tuple[object, object]]) -> None:
    _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
    filters.append(Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES))


def compute_is_crossed(side: str, price: Decimal, best_opposing_price: Optional[Decimal]) -> bool:
    """Returns True if this order crosses the market.

    A BID crosses when its price >= best available ASK price (buyer willing to
    pay at or above what sellers are asking -- immediate execution possible).
    An ASK crosses when its price <= best available BID price (seller willing
    to accept at or below what buyers are offering).
    """
    if best_opposing_price is None:
        return False
    if side == "BID":
        return price >= best_opposing_price
    return price <= best_opposing_price


def _order_key(order: OrderBookOrder) -> tuple[UUID, UUID | None, str]:
    return (order.product_id, order.delivery_point_id, normalize_availability_window(order.availability_window))


def _normalize_query_window(value: str | None) -> str | None:
    if value is None:
        return None
    try:
        return normalize_availability_window(value)
    except ValueError as exc:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail=str(exc)) from exc


def _supplier_metadata_payload(source: object) -> dict[str, object]:
    return {
        field: getattr(source, field)
        for field in SUPPLIER_METADATA_FIELDS
    }


def _supplier_metadata_fields_present(source: BaseModel) -> set[str]:
    return set(getattr(source, "model_fields_set", set())) & set(SUPPLIER_METADATA_FIELDS)


def _require_supplier_certification(
    *,
    certification_declared: bool,
    certification_scheme: str | None,
) -> None:
    if not certification_declared:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="ASK orders require an explicit certification declaration",
        )

    if not certification_scheme or not certification_scheme.strip():
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="ASK orders require a certification scheme",
        )


async def _benchmark_payload(db: AsyncSession, order: OrderBookOrder) -> dict[str, object]:
    if order.off_spec:
        return {
            "benchmark_price_per_mt_usd": None,
            "premium_discount_per_mt_usd": None,
            "benchmark_source": None,
        }

    quote = await get_benchmark_quote(
        db,
        market_product=order.market_product,
        delivery_point_id=order.delivery_point_id,
        availability_window=order.availability_window,
    )
    if quote is None:
        return {
            "benchmark_price_per_mt_usd": None,
            "premium_discount_per_mt_usd": None,
            "benchmark_source": None,
        }

    return {
        "benchmark_price_per_mt_usd": quote.benchmark_price_per_mt_usd,
        "premium_discount_per_mt_usd": compute_premium_discount(
            listing_price_per_mt_usd=order.price_per_mt_usd,
            benchmark_price_per_mt_usd=quote.benchmark_price_per_mt_usd,
        ),
        "benchmark_source": quote.source,
    }


async def _order_response(db: AsyncSession, order: OrderBookOrder, *, is_crossed: bool = False) -> OrderResponse:
    payload = OrderResponse.model_validate(order, from_attributes=True).model_copy(
        update={
            "is_crossed": is_crossed,
            **(await _benchmark_payload(db, order)),
        }
    )
    return payload


async def _order_my_response(db: AsyncSession, order: OrderBookOrder) -> OrderMyResponse:
    item = OrderMyResponse.model_validate(order, from_attributes=True).model_copy(
        update=await _benchmark_payload(db, order)
    )
    if order.side == OrderSide.BID:
        item.trade_count = len(order.bid_trades)
    else:
        item.trade_count = len(order.ask_trades)
    return item


async def _load_best_opposing_prices(
    db: AsyncSession,
    orders: list[OrderBookOrder],
    *,
    opposing_side: OrderSide,
) -> dict[tuple[UUID, UUID | None, str], Decimal]:
    if not orders:
        return {}

    key_filters = []
    for order in orders:
        filters = [
            OrderBookOrder.product_id == order.product_id,
            OrderBookOrder.availability_window == normalize_availability_window(order.availability_window),
        ]
        if order.delivery_point_id is None:
            filters.append(OrderBookOrder.delivery_point_id.is_(None))
        else:
            filters.append(OrderBookOrder.delivery_point_id == order.delivery_point_id)
        key_filters.append(and_(*filters))

    aggregate_fn = func.min if opposing_side == OrderSide.ASK else func.max
    stmt = (
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window,
            aggregate_fn(OrderBookOrder.price_per_mt_usd).label("best_price"),
        )
        .where(
            OrderBookOrder.side == opposing_side,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            or_(*key_filters),
        )
        .group_by(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window,
        )
    )
    result = await db.execute(stmt)
    return {
        (
            row.product_id,
            row.delivery_point_id,
            normalize_availability_window(str(row.availability_window)),
        ): row.best_price
        for row in result.all()
        if row.best_price is not None
    }


# ============== Static routes (must come before parametric /{order_id}) ==============


@router.get("/bids", response_model=PaginatedResponse[OrderResponse])
async def list_bids(
    product_id: Optional[UUID] = Query(None, description="Filter by product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type (e.g. Methanol, LNG)"),
    region: Optional[str] = Query(None, description="Filter by region"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open BID orders with pagination.
    """
    # Build shared filter conditions
    filters = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.side == OrderSide.BID,
    ]
    joins = []
    _apply_public_marketplace_scope(filters, joins)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if fuel_type:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(Product.fuel_type == fuel_type)
    if delivery_point_id:
        filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    if region:
        if not any(j[0] == DeliveryPoint for j in joins):
            joins.append((DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id))
        filters.append(or_(DeliveryPoint.region == region, DeliveryPoint.name == region))
    normalized_window = _normalize_query_window(availability_window)
    if normalized_window:
        filters.append(OrderBookOrder.availability_window == normalized_window)

    # Count query (same filters, no pagination)
    count_query = select(func.count(OrderBookOrder.id))
    for join_target, join_cond in joins:
        count_query = count_query.join(join_target, join_cond)
    count_query = count_query.where(*filters)
    total = (await db.execute(count_query)).scalar()

    # Data query with pagination
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
    )
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).order_by(OrderBookOrder.created_at.desc()).offset(skip).limit(limit)
    result = await db.execute(query)
    orders = result.unique().scalars().all()

    best_ask_prices = await _load_best_opposing_prices(db, orders, opposing_side=OrderSide.ASK)

    items = []
    for order in orders:
        items.append(
            await _order_response(
                db,
                order,
                is_crossed=compute_is_crossed(
                    "BID",
                    order.price_per_mt_usd,
                    best_ask_prices.get(_order_key(order)),
                ),
            )
        )

    return PaginatedResponse(items=items, total=total, skip=skip, limit=limit)


@router.get("/asks", response_model=PaginatedResponse[OrderResponse])
async def list_asks(
    product_id: Optional[UUID] = Query(None, description="Filter by product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type (e.g. Methanol, LNG)"),
    region: Optional[str] = Query(None, description="Filter by region"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open ASK orders with pagination.
    """
    # Build shared filter conditions
    filters = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        OrderBookOrder.side == OrderSide.ASK,
    ]
    joins = []
    _apply_public_marketplace_scope(filters, joins)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if fuel_type:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(Product.fuel_type == fuel_type)
    if delivery_point_id:
        filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    if region:
        if not any(j[0] == DeliveryPoint for j in joins):
            joins.append((DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id))
        filters.append(or_(DeliveryPoint.region == region, DeliveryPoint.name == region))
    normalized_window = _normalize_query_window(availability_window)
    if normalized_window:
        filters.append(OrderBookOrder.availability_window == normalized_window)

    # Count query (same filters, no pagination)
    count_query = select(func.count(OrderBookOrder.id))
    for join_target, join_cond in joins:
        count_query = count_query.join(join_target, join_cond)
    count_query = count_query.where(*filters)
    total = (await db.execute(count_query)).scalar()

    # Data query with pagination
    query = (
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
    )
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).order_by(OrderBookOrder.created_at.desc()).offset(skip).limit(limit)
    result = await db.execute(query)
    orders = result.unique().scalars().all()

    best_bid_prices = await _load_best_opposing_prices(db, orders, opposing_side=OrderSide.BID)

    items = []
    for order in orders:
        items.append(
            await _order_response(
                db,
                order,
                is_crossed=compute_is_crossed(
                    "ASK",
                    order.price_per_mt_usd,
                    best_bid_prices.get(_order_key(order)),
                ),
            )
        )

    return PaginatedResponse(items=items, total=total, skip=skip, limit=limit)


@router.get("/with-ci", response_model=list[OrderResponseWithCI])
async def list_orders_with_ci(
    product_id: Optional[UUID] = Query(None),
    delivery_point_id: Optional[UUID] = Query(None),
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
    if product_id:
        query = query.where(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        query = query.where(OrderBookOrder.delivery_point_id == delivery_point_id)
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
        base_resp = await _order_response(db, order)
        resp = OrderResponseWithCI.model_validate(base_resp.model_dump())
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
        result_list.append(await _order_my_response(db, order))

    return result_list


@router.get("/my/latest-ask-template", response_model=Optional[SupplierListingTemplateResponse])
async def latest_supplier_listing_template(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    result = await db.execute(
        select(OrderBookOrder)
        .where(
            OrderBookOrder.organization_id == current_user.organization_id,
            OrderBookOrder.side == OrderSide.ASK,
        )
        .order_by(OrderBookOrder.created_at.desc())
        .limit(1)
    )
    latest_ask = result.scalars().first()
    if latest_ask is None:
        return None

    payload = SupplierListingTemplateResponse(
        product_id=latest_ask.product_id,
        delivery_point_id=latest_ask.delivery_point_id,
        quantity_mt=latest_ask.quantity_mt,
        price_per_mt_usd=latest_ask.price_per_mt_usd,
        availability_window=latest_ask.availability_window,
        certifications=list(latest_ask.certifications or []),
        certification_declared=latest_ask.certification_declared,
        certification_scheme=latest_ask.certification_scheme,
        specification_standard=latest_ask.specification_standard,
        msds_available=latest_ask.msds_available,
        carbon_intensity_gco2_mj=latest_ask.carbon_intensity_gco2_mj,
        carbon_intensity_method=latest_ask.carbon_intensity_method,
        feedstock=latest_ask.feedstock,
        origin=latest_ask.origin,
        off_spec=False,
        off_spec_notes=None,
    )
    return payload


@router.get("/aggregated", response_model=list[AggregatedOrderbookResponse])
async def list_aggregated_orderbook(
    db: AsyncSession = Depends(get_db),
):
    """
    Market data aggregated by product, delivery point, and side.
    """
    query = (
        select(
            OrderBookOrder.product_id,
            Product.name.label("product_name"),
            Product.fuel_type.label("fuel_type"),
            OrderBookOrder.delivery_point_id,
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            OrderBookOrder.side,
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_quantity"),
            func.count(OrderBookOrder.id).label("order_count"),
        )
        .join(Product, OrderBookOrder.product_id == Product.id)
        .outerjoin(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .where(
            OrderBookOrder.status == OrderBookStatus.OPEN,
            Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES),
        )
        .group_by(
            OrderBookOrder.product_id, Product.name, Product.fuel_type,
            OrderBookOrder.delivery_point_id, DeliveryPoint.name, DeliveryPoint.region,
            OrderBookOrder.side,
        )
        .order_by(Product.name, DeliveryPoint.name, OrderBookOrder.side)
    )

    result = await db.execute(query)
    rows = result.all()

    aggregated_data = []
    for row in rows:
        aggregated_data.append(
            AggregatedOrderbookResponse(
                product_id=row.product_id,
                product_name=row.product_name or "",
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name or "",
                region=row.region or "",
                side=row.side,
                min_price=row.min_price,
                max_price=row.max_price,
                total_quantity=row.total_quantity,
                order_count=row.order_count,
            )
        )

    return aggregated_data


@router.get("/products", response_model=list[str])
async def list_active_products(db: AsyncSession = Depends(get_db)):
    """
    Get distinct product names from open orders.
    """
    query = (
        select(Product.name)
        .join(OrderBookOrder, OrderBookOrder.product_id == Product.id)
        .where(
            OrderBookOrder.status == OrderBookStatus.OPEN,
            Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES),
        )
        .distinct()
    )
    result = await db.execute(query)
    return list(result.scalars().all())


@router.get("/regions", response_model=list[str])
async def list_regions(db: AsyncSession = Depends(get_db)):
    """
    Get distinct regions from open orders via delivery points.
    """
    query = (
        select(DeliveryPoint.region)
        .join(OrderBookOrder, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .join(Product, OrderBookOrder.product_id == Product.id)
        .where(
            OrderBookOrder.status == OrderBookStatus.OPEN,
            Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES),
        )
        .distinct()
    )
    result = await db.execute(query)
    regions = result.scalars().all()
    return list(regions)


@router.get("/fuel-types", response_model=list[str])
async def list_fuel_types(db: AsyncSession = Depends(get_db)):
    """
    Get distinct fuel types from open orders via products.
    """
    query = (
        select(Product.fuel_type)
        .join(OrderBookOrder, OrderBookOrder.product_id == Product.id)
        .where(
            OrderBookOrder.status == OrderBookStatus.OPEN,
            Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES),
        )
        .distinct()
    )
    result = await db.execute(query)
    fuel_types = result.scalars().all()
    return sorted(fuel_types)


# ============== List all + CRUD (parametric routes last) ==============


@router.get("", response_model=list[OrderResponse])
async def list_orders(
    product_id: Optional[UUID] = Query(None, description="Filter by product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point"),
    side: Optional[OrderSide] = Query(None, description="Filter by side (BID or ASK)"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open and partially filled orders (bids + asks).
    """
    query = (
        select(OrderBookOrder)
        .join(Product, OrderBookOrder.product_id == Product.id)
        .options(selectinload(OrderBookOrder.organization))
        .where(
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES),
        )
    )

    if product_id:
        query = query.where(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        query = query.where(OrderBookOrder.delivery_point_id == delivery_point_id)
    if side:
        query = query.where(OrderBookOrder.side == side)
    normalized_window = _normalize_query_window(availability_window)
    if normalized_window:
        query = query.where(OrderBookOrder.availability_window == normalized_window)

    query = query.order_by(OrderBookOrder.created_at.desc())
    result = await db.execute(query)
    orders = result.scalars().all()
    return [await _order_response(db, order) for order in orders]


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

    if order_data.side != OrderSide.ASK:
        supplied_metadata_fields = _supplier_metadata_fields_present(order_data)
        if supplied_metadata_fields:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Supplier metadata is only allowed for ASK orders: {', '.join(sorted(supplied_metadata_fields))}",
            )
    else:
        _require_supplier_certification(
            certification_declared=order_data.certification_declared,
            certification_scheme=order_data.certification_scheme,
        )

    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    # Validate product_id exists
    product_result = await db.execute(
        select(Product).where(Product.id == order_data.product_id)
    )
    product = product_result.scalars().first()
    if not product:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid product_id",
        )

    # Validate delivery_point_id if provided
    if order_data.delivery_point_id:
        dp_result = await db.execute(
            select(DeliveryPoint).where(DeliveryPoint.id == order_data.delivery_point_id)
        )
        if not dp_result.scalars().first():
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid delivery_point_id",
            )

    new_order = OrderBookOrder(
        organization_id=current_user.organization_id,
        side=order_data.side,
        product_id=order_data.product_id,
        delivery_point_id=order_data.delivery_point_id,
        port_id=order_data.port_id,
        vessel_id=order_data.vessel_id,
        quantity_mt=order_data.quantity_mt,
        remaining_quantity_mt=order_data.quantity_mt,
        price_per_mt_usd=order_data.price_per_mt_usd,
        availability_window=order_data.availability_window,
        expires_at=order_data.expires_at,
    )

    if order_data.side == OrderSide.ASK:
        new_order.certifications = order_data.certifications
        for field, value in _supplier_metadata_payload(order_data).items():
            setattr(new_order, field, value)

    db.add(new_order)
    await db.flush()  # Get the order ID without committing

    # --- Match-on-insert: scan for crossing orders ---
    matched_trades: list = []
    from app.config import settings
    if settings.AUTO_MATCHING_ENABLED:
        from app.services.matching_engine import match_order
        matched_trades = await match_order(db, new_order, is_anonymous=order_data.is_anonymous)

    await db.commit()

    # Publish events for any auto-matched trades
    if matched_trades:
        for trade in matched_trades:
            await event_bus.publish("trades", "trade_auto_matched", {
                "trade_id": str(trade.id),
                "product_name": new_order.product_name,
                "fuel_type": new_order.fuel_type,
                "quantity": str(trade.quantity_mt),
                "price": str(trade.price_per_mt_usd),
                "is_anonymous": trade.is_anonymous,
            })
        await event_bus.publish("orderbook", "orders_matched", {
            "order_id": str(new_order.id),
            "matches": len(matched_trades),
        })

    await db.refresh(new_order)

    # Re-fetch with eager loading so tier_label computed property works
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == new_order.id)
    )
    new_order = result.scalars().first()

    # Emit SSE event for new order
    await event_bus.publish("orderbook", "order_created", {
        "id": str(new_order.id),
        "side": new_order.side.value,
        "product_name": new_order.product_name,
        "fuel_type": new_order.fuel_type,
        "region": new_order.region,
        "price": str(new_order.price_per_mt_usd),
        "quantity": str(new_order.remaining_quantity_mt),
    })

    return await _order_response(db, new_order)


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
        .with_for_update()
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
    if order.side != OrderSide.ASK:
        for field in SUPPLIER_METADATA_FIELDS:
            update_dict.pop(field, None)
    else:
        _require_supplier_certification(
            certification_declared=bool(update_dict.get("certification_declared", order.certification_declared)),
            certification_scheme=(update_dict.get("certification_scheme", order.certification_scheme)),
        )

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
        if new_remaining == 0:
            update_dict["status"] = OrderBookStatus.FILLED

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

    return await _order_response(db, order)


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
        select(OrderBookOrder).where(OrderBookOrder.id == order_id).with_for_update()
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

    # Emit SSE event for cancelled order
    await event_bus.publish("orderbook", "order_cancelled", {
        "id": str(order.id),
        "side": order.side.value,
        "product_name": order.product_name,
        "fuel_type": order.fuel_type,
        "region": order.region,
    })

    return None
