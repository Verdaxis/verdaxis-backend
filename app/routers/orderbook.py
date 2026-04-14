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
from app.models.catalog import Product, DeliveryPoint, MarketProduct
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
from app.services.watchlist_events import emit_order_created, emit_order_updated, emit_pin_updated, emit_slice_state_changed, _best_slice_price
from app.services.execution_policy import normalize_certification_scheme

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
APPROVED_MARKET_PRODUCTS = tuple(member.value for member in MarketProduct)
EXECUTION_QUALIFIER_FIELDS = ("certification_scheme",)
ASK_ONLY_METADATA_FIELDS = tuple(field for field in SUPPLIER_METADATA_FIELDS if field not in EXECUTION_QUALIFIER_FIELDS)
REQUIRED_ASK_METADATA_FIELDS = (
    "specification_standard",
    "msds_available",
    "carbon_intensity_gco2_mj",
    "feedstock",
    "origin",
)


def _ensure_join(joins: list[tuple[object, object]], target: object, condition: object) -> None:
    if not any(existing_target == target for existing_target, _ in joins):
        joins.append((target, condition))


def _apply_public_marketplace_scope(
    filters: list[object],
    joins: list[tuple[object, object]],
    *,
    include_off_spec: bool = False,
) -> None:
    _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
    filters.append(Product.fuel_type.in_(APPROVED_MARKETPLACE_FUEL_TYPES))
    if not include_off_spec:
        filters.append(OrderBookOrder.off_spec.is_(False))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.certification_declared.is_(True)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.specification_standard, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.msds_available.is_(True)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.carbon_intensity_gco2_mj.is_not(None)))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.feedstock, ""))) > 0))
    filters.append(or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.origin, ""))) > 0))


def _normalize_market_product_query(value: MarketProduct | str | None) -> str | None:
    if value is None or not isinstance(value, (str, MarketProduct)):
        return None
    normalized = value.value if isinstance(value, MarketProduct) else value
    if normalized not in APPROVED_MARKET_PRODUCTS:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Invalid market_product")
    return normalized


def _market_product_filter_condition(market_product: str):
    lowered_name = func.lower(Product.name)
    lowered_type = func.lower(Product.fuel_type)
    lowered_grade = func.lower(Product.fuel_grade)

    if market_product == MarketProduct.BIO_METHANOL.value:
        return or_(
            lowered_name.in_(["bio methanol", "methanol green"]),
            and_(lowered_type == "methanol", lowered_grade.in_(["bio", "green"])),
        )
    if market_product == MarketProduct.E_METHANOL.value:
        return or_(
            lowered_name == "e-methanol",
            and_(lowered_type == "methanol", lowered_grade.in_(["e", "synthetic"])),
        )
    if market_product == MarketProduct.BIO_ETHANOL.value:
        return or_(
            lowered_name.in_(["bio ethanol", "ethanol green"]),
            and_(lowered_type == "ethanol", lowered_grade.in_(["bio", "green"])),
        )
    if market_product == MarketProduct.SYNTHETIC_ETHANOL.value:
        return or_(
            lowered_name == "synthetic ethanol",
            and_(lowered_type == "ethanol", lowered_grade == "synthetic"),
        )
    raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Invalid market_product")


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


async def _watchlist_before_state(db: AsyncSession, order: OrderBookOrder) -> dict[str, object]:
    return {
        "price_per_mt_usd": order.price_per_mt_usd,
        "remaining_quantity_mt": order.remaining_quantity_mt,
        "status": order.status,
        "availability_window": normalize_availability_window(order.availability_window),
        "delivery_point_id": order.delivery_point_id,
        "market_product": order.market_product,
        "slice_best_price_per_mt_usd": await _best_slice_price(
            db,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=order.availability_window,
            side=order.side,
        ),
    }


def _ask_only_metadata_fields_present(source: BaseModel) -> set[str]:
    return set(getattr(source, "model_fields_set", set())) & set(ASK_ONLY_METADATA_FIELDS)


def _require_execution_certification_scheme(
    *,
    certification_scheme: str | None,
    detail_prefix: str,
) -> None:
    if not certification_scheme:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"{detail_prefix} require a certification scheme",
        )


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

    if not certification_scheme:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="ASK orders require a certification scheme",
        )


def _require_supplier_metadata(
    *,
    specification_standard: str | None,
    msds_available: bool,
    carbon_intensity_gco2_mj: Decimal | None,
    feedstock: str | None,
    origin: str | None,
) -> None:
    missing: list[str] = []
    if not (specification_standard or '').strip():
        missing.append('specification_standard')
    if not msds_available:
        missing.append('msds_available')
    if carbon_intensity_gco2_mj is None:
        missing.append('carbon_intensity_gco2_mj')
    if not (feedstock or '').strip():
        missing.append('feedstock')
    if not (origin or '').strip():
        missing.append('origin')
    if missing:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"ASK orders require supplier details: {', '.join(missing)}",
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
    market_product: Optional[MarketProduct] = Query(None, description="Filter by canonical market product"),
    region: Optional[str] = Query(None, description="Filter by region"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
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
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if fuel_type:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(Product.fuel_type == fuel_type)
    normalized_market_product = _normalize_market_product_query(market_product)
    if normalized_market_product:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(_market_product_filter_condition(normalized_market_product))
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
    market_product: Optional[MarketProduct] = Query(None, description="Filter by canonical market product"),
    region: Optional[str] = Query(None, description="Filter by region"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
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
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if fuel_type:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(Product.fuel_type == fuel_type)
    normalized_market_product = _normalize_market_product_query(market_product)
    if normalized_market_product:
        _ensure_join(joins, Product, OrderBookOrder.product_id == Product.id)
        filters.append(_market_product_filter_condition(normalized_market_product))
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
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    db: AsyncSession = Depends(get_db),
):
    """
    List executable orders enriched with CI-adjusted pricing.
    Orders that have carbon_intensity and energy_density populated
    will include the ci_adjusted_price object.
    """
    filters = [OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED])]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    if side:
        filters.append(OrderBookOrder.side == side)

    query = select(OrderBookOrder).options(selectinload(OrderBookOrder.organization))
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).order_by(OrderBookOrder.created_at.desc())

    result = await db.execute(query)
    orders = result.unique().scalars().all()

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
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    db: AsyncSession = Depends(get_db),
):
    """
    Market data aggregated by product, delivery point, and side.
    """
    filters = [OrderBookOrder.status == OrderBookStatus.OPEN]
    joins = [(Product, OrderBookOrder.product_id == Product.id)]
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)

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
        .where(*filters)
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
            OrderBookOrder.off_spec.is_(False),
            or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))) > 0),
            or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.certification_declared.is_(True)),
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
            OrderBookOrder.off_spec.is_(False),
            or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))) > 0),
            or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.certification_declared.is_(True)),
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
            OrderBookOrder.off_spec.is_(False),
            or_(OrderBookOrder.side != OrderSide.ASK, func.length(func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))) > 0),
            or_(OrderBookOrder.side != OrderSide.ASK, OrderBookOrder.certification_declared.is_(True)),
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
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    db: AsyncSession = Depends(get_db),
):
    """
    List all open and partially filled executable orders (bids + asks).
    """
    filters = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    if side:
        filters.append(OrderBookOrder.side == side)
    normalized_window = _normalize_query_window(availability_window)
    if normalized_window:
        filters.append(OrderBookOrder.availability_window == normalized_window)

    query = select(OrderBookOrder).options(selectinload(OrderBookOrder.organization))
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).order_by(OrderBookOrder.created_at.desc())
    result = await db.execute(query)
    orders = result.unique().scalars().all()
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

    normalized_certification_scheme = normalize_certification_scheme(order_data.certification_scheme)

    if order_data.side != OrderSide.ASK:
        supplied_metadata_fields = _ask_only_metadata_fields_present(order_data)
        if supplied_metadata_fields:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=f"Supplier metadata is only allowed for ASK orders: {', '.join(sorted(supplied_metadata_fields))}",
            )
    else:
        _require_supplier_certification(
            certification_declared=order_data.certification_declared,
            certification_scheme=normalized_certification_scheme,
        )
        _require_supplier_metadata(
            specification_standard=order_data.specification_standard,
            msds_available=order_data.msds_available,
            carbon_intensity_gco2_mj=order_data.carbon_intensity_gco2_mj,
            feedstock=order_data.feedstock,
            origin=order_data.origin,
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
        certification_scheme=normalized_certification_scheme,
    )

    if order_data.side == OrderSide.ASK:
        new_order.certifications = order_data.certifications
        for field, value in _supplier_metadata_payload(order_data).items():
            setattr(new_order, field, value)

    db.add(new_order)
    await db.flush()  # Get the order ID without committing

    previous_best_price = await _best_slice_price(
        db,
        market_product_code=new_order.market_product,
        delivery_point_id=new_order.delivery_point_id,
        availability_window_code=new_order.availability_window,
        side=new_order.side,
    )
    if previous_best_price == new_order.price_per_mt_usd:
        comparison_stmt = (
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.product))
            .where(
                OrderBookOrder.id != new_order.id,
                OrderBookOrder.delivery_point_id == new_order.delivery_point_id,
                OrderBookOrder.side == new_order.side,
                OrderBookOrder.availability_window == normalize_availability_window(new_order.availability_window),
                OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
                OrderBookOrder.off_spec.is_(False),
            )
        )
        comparison_orders = (await db.execute(comparison_stmt)).scalars().all()
        prices = [candidate.price_per_mt_usd for candidate in comparison_orders if candidate.market_product == new_order.market_product]
        if prices:
            previous_best_price = min(prices) if new_order.side == OrderSide.ASK else max(prices)
        else:
            previous_best_price = None

    resting_side = OrderSide.ASK if new_order.side == OrderSide.BID else OrderSide.BID
    resting_side_previous_best_price = await _best_slice_price(
        db,
        market_product_code=new_order.market_product,
        delivery_point_id=new_order.delivery_point_id,
        availability_window_code=new_order.availability_window,
        side=resting_side,
    )

    # --- Match-on-insert: scan for crossing orders ---
    matched_trades: list = []
    from app.config import settings
    if settings.AUTO_MATCHING_ENABLED:
        from app.services.matching_engine import match_order
        matched_trades = await match_order(db, new_order, is_anonymous=order_data.is_anonymous)

    event_result = await db.execute(
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(OrderBookOrder.id == new_order.id)
    )
    new_order = event_result.scalars().first()
    if new_order is not None:
        await emit_order_created(db, new_order, previous_best_price=previous_best_price)
        if matched_trades:
            matched_quantity_by_resting_order: dict[UUID, Decimal] = {}
            for trade in matched_trades:
                resting_order_id = trade.ask_order_id if new_order.side == OrderSide.BID else trade.bid_order_id
                matched_quantity_by_resting_order[resting_order_id] = matched_quantity_by_resting_order.get(resting_order_id, Decimal('0')) + trade.quantity_mt

            resting_orders_result = await db.execute(
                select(OrderBookOrder)
                .options(
                    selectinload(OrderBookOrder.organization),
                    selectinload(OrderBookOrder.product),
                    selectinload(OrderBookOrder.delivery_point),
                )
                .where(OrderBookOrder.id.in_(matched_quantity_by_resting_order.keys()))
            )
            resting_orders = resting_orders_result.scalars().all()
            for resting_order in resting_orders:
                matched_quantity = matched_quantity_by_resting_order.get(resting_order.id, Decimal('0'))
                before_remaining = resting_order.remaining_quantity_mt + matched_quantity
                before_status = OrderBookStatus.OPEN if before_remaining == resting_order.quantity_mt else OrderBookStatus.PARTIALLY_FILLED
                await emit_pin_updated(
                    db,
                    before={
                        'price_per_mt_usd': resting_order.price_per_mt_usd,
                        'remaining_quantity_mt': before_remaining,
                        'status': before_status,
                    },
                    order=resting_order,
                )

            representative_resting_order = resting_orders[0] if resting_orders else None
            if representative_resting_order is not None:
                await emit_slice_state_changed(
                    db,
                    market_product_code=representative_resting_order.market_product,
                    delivery_point_id=representative_resting_order.delivery_point_id,
                    availability_window_code=representative_resting_order.availability_window,
                    side=representative_resting_order.side,
                    before_best_price=resting_side_previous_best_price,
                    quiet_order_id=representative_resting_order.id,
                )

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
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
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
    if "certification_scheme" in update_dict:
        update_dict["certification_scheme"] = normalize_certification_scheme(update_dict["certification_scheme"])
    if order.side != OrderSide.ASK:
        for field in ASK_ONLY_METADATA_FIELDS:
            update_dict.pop(field, None)
    else:
        _require_supplier_certification(
            certification_declared=bool(update_dict.get("certification_declared", order.certification_declared)),
            certification_scheme=update_dict.get("certification_scheme", order.certification_scheme),
        )
        _require_supplier_metadata(
            specification_standard=update_dict.get("specification_standard", order.specification_standard),
            msds_available=bool(update_dict.get("msds_available", order.msds_available)),
            carbon_intensity_gco2_mj=update_dict.get("carbon_intensity_gco2_mj", order.carbon_intensity_gco2_mj),
            feedstock=update_dict.get("feedstock", order.feedstock),
            origin=update_dict.get("origin", order.origin),
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

    before_state = await _watchlist_before_state(db, order)

    for field, value in update_dict.items():
        setattr(order, field, value)

    await emit_order_updated(db, before=before_state, order=order)

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
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
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
            detail="You can only cancel your own orders",
        )

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only cancel orders with OPEN or PARTIALLY_FILLED status",
        )

    before_state = await _watchlist_before_state(db, order)
    order.status = OrderBookStatus.CANCELLED
    await emit_order_updated(db, before=before_state, order=order)
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
