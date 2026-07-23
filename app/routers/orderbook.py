import hashlib

from fastapi import APIRouter, Body, Depends, Header, HTTPException, Request, status, Query
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import (
    ORDER_CANCELLED,
    ORDER_CREATED,
    ORDER_UPDATED,
    MARKET_SUPPORT_AUTHORIZATION_CREATED,
)
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func, or_, and_
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import selectinload
from typing import Annotated, Optional
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from uuid import UUID

from app.database import get_db
from app.config import settings
from app.routers.auth_simple import get_authenticated_user, get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.models.user import OrganizationProvenance, User, UserRole
from app.models.orderbook import OrderBookOrder, OrderSide, OrderBookStatus, OrderCreationMethod
from app.models.market_support import (
    MarketSupportAuthorization,
    MarketSupportAuthorizationStatus,
    MarketSupportContext as MarketSupportContextModel,
)
from app.models.notification import NotificationType
from app.market_catalog import APPROVED_MARKET_PRODUCTS, MarketProduct
from app.models.catalog import Product, DeliveryPoint
from app.schemas.orderbook import (
    OrderCreate,
    OrderCancelRequest,
    OrderUpdate,
    OrderResponse,
    OrderMyResponse,
    SupplierListingTemplateResponse,
    AggregatedOrderbookResponse,
    OrderResponseWithCI,
)
from app.schemas.pagination import PaginatedResponse
from app.schemas.market_activity import MarketDemoStatus, MarketScope, MarketSourceKind
from app.services.ci_pricing import calculate_ci_adjusted_price
from app.services.activity import order_activity_provenance
from app.services.market_events import (
    commit_market_events,
    enqueue_market_events,
    participant_market_event,
)
from app.services.market_transactions import retry_market_transaction
from app.services.org_notifications import notify_org_users_batched
from app.services import market_transactions
from app.services.availability_windows import (
    is_tradable_availability_window,
    normalize_availability_window,
)
from pydantic import BaseModel
from app.services.benchmarks import compute_premium_discount
from app.services.watchlist_events import emit_order_created, emit_order_updated, _best_slice_price
from app.services.execution_policy import normalize_certification_scheme
from app.services.market_locks import acquire_market_slice_lock, acquire_market_slice_locks
from app.services.idempotency import (
    ORDER_CREATE_OPERATION,
    acquire_idempotency_lock,
    idempotency_request_hash,
)
from app.services.inventory_reservations import release_inventory, reserve_inventory
from app.services.provenance import snapshot_organization_provenance
from app.services.market_provenance import (
    canonical_availability_window_clause,
    order_market_provenance,
    public_order_evidence_clause,
)
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    canonical_delivery_point_clause,
    canonical_market_product_expression,
    current_public_order_clause,
    public_order_collection_provenance_clause,
)
from app.services.live_benchmarks import (
    LiveBenchmarkKey,
    get_live_slice_benchmark_price,
    rebuild_live_slice_benchmarks_for_keys,
)
from app.services.behavioral_analytics import (
    order_created_event,
    track_analytics_event,
    trade_created_event,
)
from app.services.auto_match_side_effects import collect_auto_match_side_effects
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)
from app.services.market_support import (
    authorization_terms_digest,
    economic_order_idempotency_payload,
    lock_support_order_parties,
    order_etag,
    require_matching_etag,
)
from app.services.request_party import (
    MARKET_SUPPORT_CONTEXT_HEADER,
    RequestPartyMode,
    lock_request_party_context,
    resolve_request_party,
)
from app.services.market_support_post_only import assess_locked_ask

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
    _ensure_join(
        joins,
        DeliveryPoint,
        OrderBookOrder.delivery_point_id == DeliveryPoint.id,
    )
    filters.extend(active_market_catalog_clauses(Product, DeliveryPoint))
    filters.append(
        canonical_availability_window_clause(OrderBookOrder.availability_window)
    )
    filters.append(current_public_order_clause(OrderBookOrder))
    filters.append(public_order_collection_provenance_clause(OrderBookOrder))
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
    if market_product not in APPROVED_MARKET_PRODUCTS:
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Invalid market_product")
    return canonical_market_product_expression(Product) == market_product


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


def _order_key(order: OrderBookOrder) -> tuple[UUID, UUID | None, str, str]:
    provenance = getattr(order.provenance, "value", order.provenance)
    return (
        order.product_id,
        order.delivery_point_id,
        normalize_availability_window(order.availability_window),
        str(provenance),
    )


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


def _audit_value(value: object) -> object:
    if hasattr(value, "value"):
        return value.value
    if isinstance(value, (Decimal, UUID)):
        return str(value)
    if isinstance(value, (date, datetime)):
        return value.isoformat()
    if isinstance(value, list):
        return [_audit_value(item) for item in value]
    return value


def _changed_fields(before: dict[str, object], after: dict[str, object]) -> dict[str, dict[str, object]]:
    changes: dict[str, dict[str, object]] = {}
    for field, before_value in before.items():
        after_value = after[field]
        if before_value != after_value:
            changes[field] = {
                "from": _audit_value(before_value),
                "to": _audit_value(after_value),
            }
    return changes


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


async def _live_slice_benchmark_price(
    db: AsyncSession,
    order: OrderBookOrder,
    *,
    cache: dict[LiveBenchmarkKey, Decimal | None] | None = None,
) -> Decimal | None:
    return await get_live_slice_benchmark_price(
        db,
        side=order.side,
        market_product=order.market_product,
        delivery_point_id=order.delivery_point_id,
        availability_window=order.availability_window,
        cache=cache,
    )


async def _benchmark_payload(
    db: AsyncSession,
    order: OrderBookOrder,
    *,
    cache: dict[LiveBenchmarkKey, Decimal | None] | None = None,
) -> dict[str, object]:
    benchmark_price = await _live_slice_benchmark_price(db, order, cache=cache)
    if benchmark_price is None:
        return {
            "benchmark_price_per_mt_usd": None,
            "premium_discount_per_mt_usd": None,
            "benchmark_source": None,
        }

    return {
        "benchmark_price_per_mt_usd": benchmark_price,
        "premium_discount_per_mt_usd": compute_premium_discount(
            listing_price_per_mt_usd=order.price_per_mt_usd,
            benchmark_price_per_mt_usd=benchmark_price,
        ),
        "benchmark_source": f"live_slice_{order.side.value.lower()}_vwap",
    }


async def _order_response(
    db: AsyncSession,
    order: OrderBookOrder,
    *,
    is_crossed: bool = False,
    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] | None = None,
) -> OrderResponse:
    payload = OrderResponse.model_validate(order, from_attributes=True).model_copy(
        update={
            "is_crossed": is_crossed,
            **order_market_provenance(order),
            **(await _benchmark_payload(db, order, cache=benchmark_cache)),
        }
    )
    return payload


async def _order_my_response(
    db: AsyncSession,
    order: OrderBookOrder,
    *,
    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] | None = None,
) -> OrderMyResponse:
    item = OrderMyResponse.model_validate(order, from_attributes=True).model_copy(
        update={
            **(await _benchmark_payload(db, order, cache=benchmark_cache)),
            "etag": order_etag(order.id, order.version),
        }
    )
    if order.side == OrderSide.BID:
        item.trade_count = len(order.bid_trades)
    else:
        item.trade_count = len(order.ask_trades)
    return item


async def _replay_belongs_to_party(
    db: AsyncSession, order: OrderBookOrder, party
) -> bool:
    """Prevent an idempotency key from replaying another principal's order."""
    if order.creation_method != party.creation_method:
        return False
    if order.owner_user_id is not None:
        if order.owner_user_id != party.accountable_principal.id:
            return False
    elif party.mode == RequestPartyMode.MARKET_SUPPORT:
        return False
    if (
        order.created_by_actor_user_id is not None
        and order.created_by_actor_user_id != party.actor.id
    ):
        return False
    if party.mode == RequestPartyMode.MARKET_SUPPORT:
        if order.support_authorization_id is None:
            return False
        authorization = (
            await db.execute(
                select(MarketSupportAuthorization).where(
                    MarketSupportAuthorization.id == order.support_authorization_id
                )
            )
        ).scalar_one_or_none()
        return bool(
            authorization is not None
            and authorization.accountable_user_id == party.accountable_principal.id
            and authorization.market_support_context_id == party.support_context_id
        )
    return True


def _reject_admin_legacy_workspace_mutation(
    request: Request, current_user: User
) -> None:
    if (
        settings.MARKET_SUPPORT_ENABLED
        and current_user.role == UserRole.ADMIN
        and not request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER)
    ):
        raise HTTPException(
            status_code=status.HTTP_410_GONE,
            detail={
                "code": "MARKET_SUPPORT_LEGACY_MUTATION_RETIRED",
                "message": "Use an active Market Support context for workspace mutations",
            },
        )


async def _load_best_opposing_prices(
    db: AsyncSession,
    orders: list[OrderBookOrder],
    *,
    opposing_side: OrderSide,
) -> dict[tuple[UUID, UUID | None, str, str], Decimal]:
    if not orders:
        return {}

    key_filters = []
    for order in orders:
        provenance = getattr(order.provenance, "value", order.provenance)
        if provenance not in {
            OrganizationProvenance.REAL.value,
            OrganizationProvenance.DEMO.value,
        }:
            continue
        filters = [
            OrderBookOrder.product_id == order.product_id,
            OrderBookOrder.availability_window == normalize_availability_window(order.availability_window),
            OrderBookOrder.provenance == provenance,
        ]
        if order.delivery_point_id is None:
            filters.append(OrderBookOrder.delivery_point_id.is_(None))
        else:
            filters.append(OrderBookOrder.delivery_point_id == order.delivery_point_id)
        key_filters.append(and_(*filters))

    if not key_filters:
        return {}

    aggregate_fn = func.min if opposing_side == OrderSide.ASK else func.max
    scope_filters: list[object] = [
        OrderBookOrder.side == opposing_side,
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        or_(*key_filters),
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(scope_filters, joins)
    stmt = (
        select(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window,
            OrderBookOrder.provenance,
            aggregate_fn(OrderBookOrder.price_per_mt_usd).label("best_price"),
        )
    )
    for join_target, join_cond in joins:
        stmt = stmt.join(join_target, join_cond)
    stmt = stmt.where(*scope_filters).group_by(
            OrderBookOrder.product_id,
            OrderBookOrder.delivery_point_id,
            OrderBookOrder.availability_window,
            OrderBookOrder.provenance,
        )
    result = await db.execute(stmt)
    return {
        (
            row.product_id,
            row.delivery_point_id,
            normalize_availability_window(str(row.availability_window)),
            getattr(row.provenance, "value", row.provenance),
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

    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] = {}

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
                benchmark_cache=benchmark_cache,
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

    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] = {}

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
                benchmark_cache=benchmark_cache,
            )
        )

    return PaginatedResponse(items=items, total=total, skip=skip, limit=limit)


@router.get("/with-ci", response_model=list[OrderResponseWithCI], deprecated=True)
async def list_orders_with_ci(
    product_id: Optional[UUID] = Query(None),
    delivery_point_id: Optional[UUID] = Query(None),
    side: Optional[OrderSide] = Query(None),
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
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
    query = (
        query.where(*filters)
        .order_by(OrderBookOrder.created_at.desc())
        .offset(skip)
        .limit(limit)
    )

    result = await db.execute(query)
    orders = result.unique().scalars().all()

    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] = {}

    enriched = []
    for order in orders:
        ci_price = None
        if order.carbon_intensity_gco2_mj and order.energy_density_mj_kg:
            ci_price = calculate_ci_adjusted_price(
                base_price_per_mt=order.price_per_mt_usd,
                carbon_intensity_gco2_mj=order.carbon_intensity_gco2_mj,
                energy_density_mj_kg=order.energy_density_mj_kg,
            )
        base_resp = await _order_response(db, order, benchmark_cache=benchmark_cache)
        resp = OrderResponseWithCI.model_validate(base_resp.model_dump())
        resp.ci_adjusted_price = ci_price
        enriched.append(resp)

    return enriched


@router.get("/my", response_model=list[OrderMyResponse], deprecated=True)
async def list_my_orders(
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    request: Request = None,
):
    """
    List current user's own orders (both bids and asks).
    """
    party = None
    if request is not None and request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER):
        party = await resolve_request_party(request, db, current_user)
    effective_organization_id = (
        party.effective_organization.id
        if party is not None and party.effective_organization is not None
        else current_user.organization_id
    )
    if not effective_organization_id:
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
        .where(OrderBookOrder.organization_id == effective_organization_id)
        .order_by(OrderBookOrder.created_at.desc())
        .offset(skip)
        .limit(limit)
    )

    result = await db.execute(query)
    orders = result.scalars().all()

    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] = {}

    result_list = []
    for order in orders:
        result_list.append(await _order_my_response(db, order, benchmark_cache=benchmark_cache))

    return result_list


@router.get("/my/latest-ask-template", response_model=Optional[SupplierListingTemplateResponse])
async def latest_supplier_listing_template(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
    request: Request = None,
):
    party = None
    if request is not None and request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER):
        party = await resolve_request_party(request, db, current_user)
    effective_organization_id = (
        party.effective_organization.id
        if party is not None and party.effective_organization is not None
        else current_user.organization_id
    )
    if not effective_organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    result = await db.execute(
        select(OrderBookOrder)
        .where(
            OrderBookOrder.organization_id == effective_organization_id,
            OrderBookOrder.side == OrderSide.ASK,
            or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > func.now()),
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
    product_id: Optional[UUID] = Query(None, description="Filter by product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    market_product: Optional[MarketProduct] = Query(None, description="Filter by canonical market product"),
    region: Optional[str] = Query(None, description="Filter by region or delivery point name"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    limit: Annotated[int, Query(ge=1, le=512)] = 256,
    db: AsyncSession = Depends(get_db),
):
    """
    Market data aggregated by product, delivery point, and side.
    """
    filters = [
        OrderBookOrder.status.in_(
            [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
        )
    ]
    joins = [(Product, OrderBookOrder.product_id == Product.id)]
    _apply_public_marketplace_scope(filters, joins, include_off_spec=include_off_spec)
    filters.append(public_order_evidence_clause(OrderBookOrder.provenance))
    if product_id:
        filters.append(OrderBookOrder.product_id == product_id)
    if delivery_point_id:
        filters.append(OrderBookOrder.delivery_point_id == delivery_point_id)
    if fuel_type:
        filters.append(Product.fuel_type == fuel_type)
    normalized_market_product = _normalize_market_product_query(market_product)
    if normalized_market_product:
        filters.append(_market_product_filter_condition(normalized_market_product))
    if region:
        filters.append(or_(DeliveryPoint.region == region, DeliveryPoint.name == region))
    normalized_window = _normalize_query_window(availability_window)
    if normalized_window:
        filters.append(OrderBookOrder.availability_window == normalized_window)

    canonical_market_product = canonical_market_product_expression(Product).label(
        "market_product"
    )
    grouped = (
        select(
            OrderBookOrder.product_id,
            Product.name.label("product_name"),
            canonical_market_product,
            Product.fuel_type.label("fuel_type"),
            OrderBookOrder.availability_window,
            OrderBookOrder.delivery_point_id,
            DeliveryPoint.name.label("delivery_point_name"),
            DeliveryPoint.region.label("region"),
            OrderBookOrder.side,
            OrderBookOrder.provenance.label("evidence_class"),
            func.min(OrderBookOrder.price_per_mt_usd).label("min_price"),
            func.max(OrderBookOrder.price_per_mt_usd).label("max_price"),
            func.sum(OrderBookOrder.remaining_quantity_mt).label("total_quantity"),
            func.count(OrderBookOrder.id).label("order_count"),
            func.max(OrderBookOrder.created_at).label("observed_at"),
        )
    )
    for join_target, join_cond in joins:
        grouped = grouped.join(join_target, join_cond)
    grouped = grouped.where(*filters).group_by(
        OrderBookOrder.product_id,
        Product.name,
        canonical_market_product,
        Product.fuel_type,
        OrderBookOrder.availability_window,
        OrderBookOrder.delivery_point_id,
        DeliveryPoint.name,
        DeliveryPoint.region,
        OrderBookOrder.side,
        OrderBookOrder.provenance,
    ).subquery("eligible_orderbook_aggregate")

    query = (
        select(
            grouped,
            func.sum(grouped.c.order_count)
            .over(
                partition_by=(
                    grouped.c.market_product,
                    grouped.c.evidence_class,
                )
            )
            .label("product_total_order_count"),
        )
        .order_by(
            grouped.c.market_product,
            grouped.c.delivery_point_name,
            grouped.c.availability_window,
            grouped.c.side,
            grouped.c.evidence_class,
        )
        .limit(limit)
    )

    result = await db.execute(query)
    rows = result.all()

    aggregated_data = []
    for row in rows:
        aggregated_data.append(
            AggregatedOrderbookResponse(
                product_id=row.product_id,
                product_name=row.product_name or "",
                market_product=row.market_product,
                fuel_type=row.fuel_type or "",
                delivery_point_id=row.delivery_point_id,
                delivery_point_name=row.delivery_point_name or "",
                availability_window=row.availability_window,
                region=row.region or "",
                side=row.side,
                min_price=row.min_price,
                max_price=row.max_price,
                total_quantity=row.total_quantity,
                order_count=row.order_count,
                product_total_order_count=row.product_total_order_count,
                evidence_class=row.evidence_class,
                source_kind=(
                    MarketSourceKind.LIVE_ORDER
                    if row.evidence_class == OrganizationProvenance.REAL.value
                    else MarketSourceKind.DEMO_SEED
                ),
                scope=MarketScope.DELIVERY_POINT,
                demo_status=(
                    MarketDemoStatus.REAL_ONLY
                    if row.evidence_class == OrganizationProvenance.REAL.value
                    else MarketDemoStatus.DEMO_ONLY
                ),
                observed_at=row.observed_at,
            )
        )

    return aggregated_data


@router.get("/products", response_model=list[str])
async def list_active_products(db: AsyncSession = Depends(get_db)):
    """
    Get distinct product names from open orders.
    """
    filters: list[object] = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        public_order_evidence_clause(OrderBookOrder.provenance),
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins)
    query = select(Product.name).select_from(OrderBookOrder)
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).distinct().order_by(Product.name)
    result = await db.execute(query)
    return list(result.scalars().all())


@router.get("/regions", response_model=list[str])
async def list_regions(db: AsyncSession = Depends(get_db)):
    """
    Get distinct regions from open orders via delivery points.
    """
    filters: list[object] = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        public_order_evidence_clause(OrderBookOrder.provenance),
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins)
    query = select(DeliveryPoint.region).select_from(OrderBookOrder)
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).distinct().order_by(DeliveryPoint.region)
    result = await db.execute(query)
    regions = result.scalars().all()
    return list(regions)


@router.get("/fuel-types", response_model=list[str])
async def list_fuel_types(db: AsyncSession = Depends(get_db)):
    """
    Get distinct fuel types from open orders via products.
    """
    filters: list[object] = [
        OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
        public_order_evidence_clause(OrderBookOrder.provenance),
    ]
    joins: list[tuple[object, object]] = []
    _apply_public_marketplace_scope(filters, joins)
    query = select(Product.fuel_type).select_from(OrderBookOrder)
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = query.where(*filters).distinct().order_by(Product.fuel_type)
    result = await db.execute(query)
    fuel_types = result.scalars().all()
    return sorted(fuel_types)


# ============== List all + CRUD (parametric routes last) ==============


@router.get("", response_model=list[OrderResponse], deprecated=True)
async def list_orders(
    product_id: Optional[UUID] = Query(None, description="Filter by product"),
    delivery_point_id: Optional[UUID] = Query(None, description="Filter by delivery point"),
    side: Optional[OrderSide] = Query(None, description="Filter by side (BID or ASK)"),
    availability_window: Optional[str] = Query(None, description="Filter by availability window"),
    include_off_spec: bool = Query(False, description="Include off-spec orders"),
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
    page: Annotated[int | None, Query(ge=1, deprecated=True)] = None,
    page_size: Annotated[int | None, Query(ge=1, le=100, deprecated=True)] = None,
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

    effective_limit = page_size if page_size is not None else limit
    effective_skip = (
        (page - 1) * effective_limit
        if page is not None
        else skip
    )
    query = select(OrderBookOrder).options(selectinload(OrderBookOrder.organization))
    for join_target, join_cond in joins:
        query = query.join(join_target, join_cond)
    query = (
        query.where(*filters)
        .order_by(OrderBookOrder.created_at.desc())
        .offset(effective_skip)
        .limit(effective_limit)
    )
    result = await db.execute(query)
    orders = result.unique().scalars().all()
    benchmark_cache: dict[LiveBenchmarkKey, Decimal | None] = {}
    return [await _order_response(db, order, benchmark_cache=benchmark_cache) for order in orders]


@router.post("", response_model=OrderResponse, status_code=status.HTTP_201_CREATED)
@retry_market_transaction()
async def create_order(
    request: Request,
    order_data: OrderCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Place a new order. BID requires BUYER role, ASK requires SUPPLIER role.
    """
    party = await resolve_request_party(request, db, current_user, operation="create_order")
    if party.mode == RequestPartyMode.SELF_SERVICE:
        if order_data.support_confirmation is not None:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Market Support confirmation requires an active support context",
            )
        if isinstance(current_user, User):
            current_user = await require_execution_eligible_user(current_user=current_user, db=db)
    else:
        if order_data.side != OrderSide.ASK:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Market Support permits ASK listings only",
            )
        if order_data.support_confirmation is None:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Final Market Support confirmation is required",
            )

    effective_organization_id = (
        party.effective_organization.id
        if party.effective_organization is not None
        else current_user.organization_id
    )
    if not effective_organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    # A committed replay belongs to the initiating tenant and is returned
    # before current role/admission or target lifecycle checks.
    idempotency_key = request.headers.get("Idempotency-Key")
    if party.mode == RequestPartyMode.MARKET_SUPPORT and not idempotency_key:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Market Support ASK orders require Idempotency-Key",
        )
    request_hash = None
    if idempotency_key:
        idempotency_key = idempotency_key.strip()
        if not idempotency_key or len(idempotency_key) > 255:
            raise HTTPException(status_code=400, detail="Idempotency-Key must be 1-255 characters")
        idempotency_payload: dict[str, object] = {
            "order": economic_order_idempotency_payload(order_data)
        }
        if (
            party.mode == RequestPartyMode.MARKET_SUPPORT
            and order_data.support_confirmation is not None
        ):
            idempotency_payload["support_confirmation"] = (
                order_data.support_confirmation.model_dump(mode="json")
            )
        request_hash = idempotency_request_hash(
            idempotency_payload
            if party.mode == RequestPartyMode.MARKET_SUPPORT
            else idempotency_payload["order"]
        )
        await acquire_idempotency_lock(
            db,
            tenant_id=effective_organization_id,
            operation=ORDER_CREATE_OPERATION,
            key=idempotency_key,
        )
        existing_result = await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(
                OrderBookOrder.organization_id == effective_organization_id,
                OrderBookOrder.idempotency_operation == ORDER_CREATE_OPERATION,
                OrderBookOrder.idempotency_key == idempotency_key,
            )
        )
        existing_order = existing_result.scalar_one_or_none()
        if existing_order is not None:
            if existing_order.idempotency_request_hash != request_hash:
                raise HTTPException(status_code=409, detail="Idempotency-Key was reused with a different request")
            if not await _replay_belongs_to_party(db, existing_order, party):
                raise HTTPException(status_code=409, detail="Idempotency-Key is bound to another request party")
            return await _order_response(db, existing_order)

    if order_data.side == OrderSide.BID and party.effective_role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can place BID orders",
        )

    if order_data.side == OrderSide.ASK and party.effective_role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can place ASK orders",
        )

    if not is_tradable_availability_window(order_data.availability_window):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Availability window is no longer open for new orders",
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

    # Validate product_id exists
    product_result = await db.execute(
        select(Product).where(
            Product.id == order_data.product_id,
            Product.is_active.is_(True),
            canonical_market_product_expression(Product).is_not(None),
        )
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
            select(DeliveryPoint).where(
                DeliveryPoint.id == order_data.delivery_point_id,
                canonical_delivery_point_clause(DeliveryPoint),
            )
        )
        if not dp_result.scalars().first():
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Invalid delivery_point_id",
            )

    # Global mutation order: idempotency lock, exact slice lock, then the
    # concrete user and organization rows. There is no existing market row
    # to lock on create; the insert follows the revalidation.
    await market_transactions.transaction_boundary_hook(
        "before_market_slice_lock",
        operation="order_admission",
        aggregate_id=effective_organization_id,
    )
    await acquire_market_slice_lock(
        db,
        side=order_data.side,
        product_id=order_data.product_id,
        delivery_point_id=order_data.delivery_point_id,
        availability_window=order_data.availability_window,
    )
    await lock_request_party_context(db, party)
    organizations = await lock_and_load_market_organizations(
        db,
        [effective_organization_id],
        actor_ownerships=(
            MarketActorOwnership(party.accountable_principal.id, effective_organization_id),
        ),
    )
    organization = organizations[effective_organization_id]

    new_order = OrderBookOrder(
        organization_id=effective_organization_id,
        owner_user_id=party.accountable_principal.id,
        created_by_actor_user_id=party.actor.id,
        creation_method=party.creation_method,
        provenance=snapshot_organization_provenance(organization),
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
        idempotency_key=idempotency_key,
        idempotency_operation=(ORDER_CREATE_OPERATION if idempotency_key else None),
        idempotency_request_hash=(request_hash if idempotency_key else None),
    )

    new_order.certifications = list(order_data.certifications)

    if order_data.side == OrderSide.ASK:
        for field, value in _supplier_metadata_payload(order_data).items():
            setattr(new_order, field, value)

    new_order.product = product
    support_authorization = None
    if party.mode == RequestPartyMode.MARKET_SUPPORT:
        confirmation = order_data.support_confirmation
        if confirmation is None or order_data.expires_at is None:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Market Support ASK orders require final confirmation and explicit expiry",
            )
        now = datetime.now(UTC)
        if order_data.port_id is not None or order_data.vessel_id is not None:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Market Support ASK orders use the canonical delivery point only",
            )
        if order_data.expires_at > now + timedelta(hours=settings.MARKET_SUPPORT_MAX_TTL_HOURS):
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail={
                    "code": "MARKET_SUPPORT_TTL_EXCEEDED",
                    "message": "Order expiry exceeds the Market Support limit",
                },
            )
        if confirmation.instruction_at > now or confirmation.instruction_at < now - timedelta(
            hours=settings.MARKET_SUPPORT_MAX_TTL_HOURS
        ):
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail={
                    "code": "MARKET_SUPPORT_INSTRUCTION_TIME_INVALID",
                    "message": "Instruction time must be recent and not in the future",
                },
            )
        if order_data.expires_at <= confirmation.instruction_at:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Order expiry must follow the instruction time",
            )
        context_result = await db.execute(
            select(MarketSupportContextModel)
            .where(MarketSupportContextModel.id == party.support_context_id)
            .with_for_update()
            .execution_options(populate_existing=True)
        )
        context_row = context_result.scalar_one()
        # The customer instruction commonly predates the admin opening this
        # support context. Context expiry limits privileged publication, not
        # the lifetime of the resulting standing ASK.
        # Assess the complete crossing set while the canonical slice is held.
        assessment = await assess_locked_ask(
            db,
            new_order,
            accountable_user=party.accountable_principal,
            organization=organization,
        )
        if assessment.indeterminate:
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail={
                    "code": "POST_ONLY_CHECK_INDETERMINATE",
                    "message": "The complete crossing set could not be assessed; retry later",
                },
            )
        if assessment.would_cross:
            raise HTTPException(
                status_code=status.HTTP_409_CONFLICT,
                detail={
                    "code": "POST_ONLY_WOULD_CROSS",
                    "message": "Market Support publication must rest without immediate execution",
                    "best_executable_opposing_price": (
                        str(assessment.best_executable_price)
                        if assessment.best_executable_price is not None
                        else None
                    ),
                },
            )
        support_authorization = MarketSupportAuthorization(
            organization_id=effective_organization_id,
            accountable_user_id=party.accountable_principal.id,
            market_support_context_id=party.support_context_id,
            product_id=new_order.product_id,
            delivery_point_id=new_order.delivery_point_id,
            availability_window=new_order.availability_window,
            quantity_mt=new_order.quantity_mt,
            price_per_mt_usd=new_order.price_per_mt_usd,
            # Replaced with the active context expiry immediately below.
            authorization_expires_at=order_data.expires_at,
            order_expires_at=order_data.expires_at,
            is_anonymous=order_data.is_anonymous,
            certifications=list(order_data.certifications),
            certification_declared=order_data.certification_declared,
            certification_scheme=normalized_certification_scheme,
            specification_standard=order_data.specification_standard,
            msds_available=order_data.msds_available,
            carbon_intensity_gco2_mj=order_data.carbon_intensity_gco2_mj,
            carbon_intensity_method=order_data.carbon_intensity_method,
            feedstock=order_data.feedstock,
            origin=order_data.origin,
            off_spec=order_data.off_spec,
            off_spec_notes=order_data.off_spec_notes,
            terms_digest=authorization_terms_digest(
                order_data.model_copy(update={"support_confirmation": None})
            ),
            evidence_reference=confirmation.external_instruction_reference,
            evidence_sha256=hashlib.sha256(
                confirmation.evidence_excerpt.encode("utf-8")
            ).hexdigest(),
            instruction_at=confirmation.instruction_at,
            acknowledge_exact_terms=confirmation.acknowledge_exact_terms,
            acknowledge_executable_standing_order=confirmation.acknowledge_executable_standing_order,
            commercial_consent_version=settings.MARKET_SUPPORT_CONSENT_VERSION,
            commercial_consent_reference=settings.MARKET_SUPPORT_CONSENT_REFERENCE,
            support_case_reference=party.support_reference,
            idempotency_key=idempotency_key,
            idempotency_request_hash=request_hash
            or idempotency_request_hash(economic_order_idempotency_payload(order_data)),
            created_by_actor_user_id=party.actor.id,
        )
        # The active context's expiry is intentionally the authorization bound.
        context_expiry = context_row.expires_at
        support_authorization.authorization_expires_at = min(
            context_expiry, order_data.expires_at
        )
        db.add(support_authorization)
        await db.flush()
        new_order.support_authorization_id = support_authorization.id
        support_authorization.status = MarketSupportAuthorizationStatus.CONSUMED
        support_authorization.consumed_at = datetime.now(UTC)
        await record_audit(
            db,
            user_id=party.actor.id,
            action=MARKET_SUPPORT_AUTHORIZATION_CREATED,
            resource_type="market_support_authorization",
            resource_id=support_authorization.id,
            changes={
                "organization_id": str(effective_organization_id),
                "accountable_user_id": str(party.accountable_principal.id),
                "support_context_id": str(party.support_context_id),
                "terms_digest": support_authorization.terms_digest,
                "evidence_reference": support_authorization.evidence_reference,
                "evidence_sha256": support_authorization.evidence_sha256,
                "instruction_at": support_authorization.instruction_at.isoformat(),
                "acknowledge_exact_terms": support_authorization.acknowledge_exact_terms,
                "acknowledge_executable_standing_order": support_authorization.acknowledge_executable_standing_order,
                "commercial_consent_version": support_authorization.commercial_consent_version,
                "status": support_authorization.status.value,
            },
            **request_audit_context(request),
        )
    previous_best_price = await _best_slice_price(
        db,
        market_product_code=new_order.market_product,
        delivery_point_id=new_order.delivery_point_id,
        availability_window_code=new_order.availability_window,
        side=new_order.side,
    )
    resting_side = OrderSide.ASK if new_order.side == OrderSide.BID else OrderSide.BID
    resting_side_previous_best_price = await _best_slice_price(
        db,
        market_product_code=new_order.market_product,
        delivery_point_id=new_order.delivery_point_id,
        availability_window_code=new_order.availability_window,
        side=resting_side,
    )
    db.add(new_order)
    try:
        await db.flush()  # Get the order ID without committing
    except IntegrityError:
        # The unique key is enforced at the actual insert, not just at the
        # later commit. The transaction lock makes this replay deterministic.
        await db.rollback()
        if not idempotency_key:
            raise
        existing_result = await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(
                OrderBookOrder.organization_id == effective_organization_id,
                OrderBookOrder.idempotency_operation == ORDER_CREATE_OPERATION,
                OrderBookOrder.idempotency_key == idempotency_key,
            )
        )
        existing_order = existing_result.scalar_one_or_none()
        if existing_order is None:
            raise
        if existing_order.idempotency_request_hash != request_hash:
            raise HTTPException(status_code=409, detail="Idempotency-Key was reused with a different request")
        if not await _replay_belongs_to_party(db, existing_order, party):
            raise HTTPException(status_code=409, detail="Idempotency-Key is bound to another request party")
        return await _order_response(db, existing_order)

    await record_audit(
        db,
        user_id=party.actor.id,
        action=ORDER_CREATED,
        resource_type="order",
        resource_id=new_order.id,
        changes={
            "side": new_order.side.value,
            "product_id": str(new_order.product_id),
            "market_product": new_order.market_product,
            "delivery_point_id": str(new_order.delivery_point_id) if new_order.delivery_point_id else None,
            "availability_window": new_order.availability_window,
            "quantity_mt": str(new_order.quantity_mt),
            "price_per_mt_usd": str(new_order.price_per_mt_usd),
            "status": new_order.status.value,
            "actor_user_id": str(party.actor.id),
            "effective_organization_id": str(effective_organization_id),
            "accountable_user_id": str(party.accountable_principal.id),
            "support_context_id": str(party.support_context_id) if party.support_context_id else None,
            "creation_method": new_order.creation_method.value,
        },
        **request_audit_context(request),
    )
    if party.mode == RequestPartyMode.MARKET_SUPPORT:
        await notify_org_users_batched(
            db,
            [
                (
                    effective_organization_id,
                    NotificationType.ORDER_UPDATE,
                    "Listing set up by Verdaxis Support",
                    "Verdaxis Support published an authorized listing for your organization.",
                    {
                        "order_id": str(new_order.id),
                        "creation_method": OrderCreationMethod.MARKET_SUPPORT.value,
                    },
                )
            ],
        )

    # --- Match-on-insert: scan for crossing orders ---
    matched_trades: list = []
    if settings.AUTO_MATCHING_ENABLED and party.mode == RequestPartyMode.SELF_SERVICE:
        from app.services.matching_engine import match_order
        matched_trades = await match_order(db, new_order, is_anonymous=order_data.is_anonymous)

    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [
            (new_order.side, product.market_product, new_order.delivery_point_id, new_order.availability_window),
            (resting_side, product.market_product, new_order.delivery_point_id, new_order.availability_window),
        ],
    )

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
    committed_events = []
    if new_order is not None:
        await emit_order_created(db, new_order, previous_best_price=previous_best_price)
        committed_events.extend(
            await collect_auto_match_side_effects(
                db,
                triggering_order=new_order,
                trades=matched_trades,
                actor_user_id=party.actor.id,
                audit_context=request_audit_context(request),
                resting_side_previous_best_price=resting_side_previous_best_price,
            )
        )
        committed_events.append(
            participant_market_event(
                event_type="order_created",
                aggregate_type="order",
                aggregate_id=new_order.id,
                participant_org_ids={
                    new_order.organization_id,
                    *(trade.buyer_id for trade in matched_trades),
                    *(trade.seller_id for trade in matched_trades),
                },
                payload={
                    **order_activity_provenance(new_order),
                    "id": str(new_order.id),
                    "side": new_order.side.value,
                    "product_name": new_order.product_name,
                    "fuel_type": new_order.fuel_type,
                    "region": new_order.region,
                    "price": str(new_order.price_per_mt_usd),
                    "quantity": str(new_order.remaining_quantity_mt),
                },
            )
        )
    await enqueue_market_events(db, committed_events)

    try:
        await db.commit()
    except IntegrityError:
        await db.rollback()
        if not idempotency_key:
            raise
        existing_result = await db.execute(
            select(OrderBookOrder)
            .options(selectinload(OrderBookOrder.organization), selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
            .where(
                OrderBookOrder.organization_id == effective_organization_id,
                OrderBookOrder.idempotency_operation == ORDER_CREATE_OPERATION,
                OrderBookOrder.idempotency_key == idempotency_key,
            )
        )
        existing_order = existing_result.scalars().first()
        if existing_order is None:
            raise
        if existing_order.idempotency_request_hash != request_hash:
            raise HTTPException(status_code=409, detail="Idempotency-Key was reused with a different request")
        if not await _replay_belongs_to_party(db, existing_order, party):
            raise HTTPException(status_code=409, detail="Idempotency-Key is bound to another request party")
        return await _order_response(db, existing_order)
    if new_order is not None:
        track_analytics_event(
            order_created_event(current_user, new_order, request=request), request=request
        )
        for trade in matched_trades:
            track_analytics_event(
                trade_created_event(current_user, order=new_order, request=request), request=request
            )

    await db.refresh(new_order)

    # Re-fetch with eager loading so tier_label computed property works
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == new_order.id)
    )
    new_order = result.scalars().first()

    return await _order_response(db, new_order)


@router.put("/{order_id}", response_model=OrderResponse)
@retry_market_transaction()
async def update_order(
    order_id: UUID,
    request: Request,
    update_data: OrderUpdate,
    current_user: User = Depends(require_execution_eligible_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Update an own order. Only allowed if status is OPEN or PARTIALLY_FILLED.
    """
    _reject_admin_legacy_workspace_mutation(request, current_user)
    result = await db.execute(
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(OrderBookOrder.id == order_id)
    )
    order = result.scalars().first()

    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found",
        )

    owns_order = (
        order.owner_user_id == current_user.id
        if order.owner_user_id is not None
        else current_user.organization_id is not None
        and order.organization_id == current_user.organization_id
    )
    if not owns_order:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only update your own orders",
        )

    if order.creation_method == OrderCreationMethod.MARKET_SUPPORT:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Assisted listings cannot be edited; cancel and create a replacement",
        )

    if (
        order.expires_at is not None
        and order.expires_at <= datetime.now(UTC)
        and order.status in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
    ):
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Order has expired")

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only update orders with OPEN or PARTIALLY_FILLED status",
        )

    update_dict = update_data.model_dump(exclude_unset=True)
    requested_window = update_dict.get("availability_window")
    if requested_window is not None and requested_window != order.availability_window:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="Executable order market identity is immutable; cancel and create a new order",
        )
    update_dict.pop("availability_window", None)
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

    # Read/validate first, then serialize every affected slice, then take the
    # order row lock. This ordering is shared with match/cancel paths.
    previous_product_id = order.product_id
    previous_delivery_point_id = order.delivery_point_id
    previous_window = order.availability_window
    economic_update = bool(
        set(update_dict)
        & (
            set(SUPPLIER_METADATA_FIELDS)
            | {
                "quantity_mt",
                "price_per_mt_usd",
                "expires_at",
                "certifications",
            }
        )
    )
    await market_transactions.transaction_boundary_hook(
        "after_market_preview",
        operation="order_update",
        aggregate_id=order_id,
    )
    await acquire_market_slice_locks(
        db,
        [
            (order.side, order.product_id, order.delivery_point_id, previous_window),
        ],
    )
    locked_result = await db.execute(
        select(OrderBookOrder)
        .options(
            selectinload(OrderBookOrder.organization),
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(OrderBookOrder.id == order_id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    order = locked_result.scalars().first()
    if order is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="Order not found")
    if order.organization_id != current_user.organization_id:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="You can only update your own orders")
    if (
        order.product_id != previous_product_id
        or order.delivery_point_id != previous_delivery_point_id
        or order.availability_window != previous_window
    ):
        await db.rollback()
        raise HTTPException(status_code=409, detail="Order slice changed; retry the update")
    if (
        order.expires_at is not None
        and order.expires_at <= datetime.now(UTC)
        and order.status in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
    ):
        await db.rollback()
        raise HTTPException(status_code=status.HTTP_400_BAD_REQUEST, detail="Order has expired")
    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        await db.rollback()
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only update orders with OPEN or PARTIALLY_FILLED status",
        )
    await lock_and_load_market_organizations(
        db,
        [current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    # Recompute quantity against the locked row; the preview may have waited
    # behind another update on the same slice.
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
        if order.inventory_item_id is not None:
            remaining_delta = new_remaining - old_remaining
            if remaining_delta > 0:
                await reserve_inventory(db, order.inventory_item_id, remaining_delta)
            elif remaining_delta < 0:
                await release_inventory(db, order.inventory_item_id, -remaining_delta)

    before_state = await _watchlist_before_state(db, order)
    previous_benchmark_key: LiveBenchmarkKey | None = (
        order.side,
        order.market_product,
        order.delivery_point_id,
        order.availability_window,
    )
    audit_fields = set(update_dict)
    audit_fields.update({"remaining_quantity_mt", "status"})
    audit_before = {
        field: getattr(order, field)
        for field in audit_fields
        if hasattr(order, field)
    }

    for field, value in update_dict.items():
        setattr(order, field, value)

    matched_trades: list = []
    resting_side_previous_best_price = None
    if economic_update and settings.AUTO_MATCHING_ENABLED:
        from app.services.matching_engine import match_order
        resting_side = OrderSide.ASK if order.side == OrderSide.BID else OrderSide.BID
        resting_side_previous_best_price = await _best_slice_price(
            db,
            market_product_code=order.market_product,
            delivery_point_id=order.delivery_point_id,
            availability_window_code=order.availability_window,
            side=resting_side,
        )
        await acquire_market_slice_lock(
            db,
            side=order.side,
            product_id=order.product_id,
            delivery_point_id=order.delivery_point_id,
            availability_window=order.availability_window,
        )
        matched_trades = await match_order(db, order, is_anonymous=True)

    audit_after = {
        field: getattr(order, field)
        for field in audit_before
    }
    audit_changes = _changed_fields(audit_before, audit_after)

    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [
            previous_benchmark_key,
            (order.side, order.market_product, order.delivery_point_id, order.availability_window),
        ],
    )
    await emit_order_updated(db, before=before_state, order=order)
    committed_events = await collect_auto_match_side_effects(
        db,
        triggering_order=order,
        trades=matched_trades,
        actor_user_id=current_user.id,
        audit_context=request_audit_context(request),
        resting_side_previous_best_price=resting_side_previous_best_price,
    )
    if audit_changes:
        await record_audit(
            db,
            user_id=current_user.id,
            action=ORDER_UPDATED,
            resource_type="order",
            resource_id=order.id,
            changes=audit_changes,
            **request_audit_context(request),
        )

    await commit_market_events(db, committed_events)
    await db.refresh(order)

    # Re-fetch with eager loading for tier_label
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.organization))
        .where(OrderBookOrder.id == order.id)
    )
    order = result.scalars().first()

    for _trade in matched_trades:
        track_analytics_event(
            trade_created_event(current_user, order=order, request=request), request=request
        )
    return await _order_response(db, order)


@router.post("/{order_id}/cancel", status_code=status.HTTP_204_NO_CONTENT)
@router.delete("/{order_id}", status_code=status.HTTP_204_NO_CONTENT)
@retry_market_transaction()
async def cancel_order(
    order_id: UUID,
    request: Request,
    current_user: User = Depends(get_authenticated_user),
    db: AsyncSession = Depends(get_db),
    if_match: Annotated[str | None, Header(alias="If-Match")] = None,
    body: OrderCancelRequest | None = Body(None),
):
    """
    Cancel an own order (soft cancel by setting status to CANCELLED).
    """
    _reject_admin_legacy_workspace_mutation(request, current_user)
    party = None
    if request.headers.get(MARKET_SUPPORT_CONTEXT_HEADER):
        party = await resolve_request_party(
            request, db, current_user, operation="cancel_order"
        )
    effective_organization_id = (
        party.effective_organization.id
        if party is not None and party.effective_organization is not None
        else current_user.organization_id
    )
    reason = body.reason if body is not None else None
    result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
        .where(OrderBookOrder.id == order_id)
    )
    order = result.scalars().first()

    if not order:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Order not found",
        )

    if order.creation_method == OrderCreationMethod.MARKET_SUPPORT and not reason:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Cancellation reason is required for Market Support listings",
        )
    if request.method == "POST" and not reason:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Cancellation reason is required",
        )

    if party is not None and party.mode == RequestPartyMode.MARKET_SUPPORT:
        if (
            order.creation_method != OrderCreationMethod.MARKET_SUPPORT
            or order.side != OrderSide.ASK
            or order.organization_id != effective_organization_id
        ):
            raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Only support-created ASK listings can be cancelled")
        authorization_context = (
            await db.execute(
                select(MarketSupportAuthorization.market_support_context_id).where(
                    MarketSupportAuthorization.id == order.support_authorization_id
                )
            )
        ).scalar_one_or_none()
        if order.support_authorization_id is None:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="The support authorization for this listing is unavailable",
            )
        owns_order = True
    else:
        owns_order = (
            order.owner_user_id == current_user.id
            if order.owner_user_id is not None
            else current_user.organization_id is not None
            and order.organization_id == current_user.organization_id
        )
    if not owns_order:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only cancel your own orders",
        )

    if order.creation_method == OrderCreationMethod.MARKET_SUPPORT:
        require_matching_etag(if_match, order_id=order.id, version=order.version)

    if order.status not in (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Can only cancel orders with OPEN or PARTIALLY_FILLED status",
        )

    locked_product_id = order.product_id
    locked_delivery_point_id = order.delivery_point_id
    locked_window = order.availability_window
    await acquire_market_slice_lock(
        db,
        side=order.side,
        product_id=order.product_id,
        delivery_point_id=order.delivery_point_id,
        availability_window=order.availability_window,
    )
    if party is not None:
        await lock_request_party_context(db, party)
    locked_result = await db.execute(
        select(OrderBookOrder)
        .options(selectinload(OrderBookOrder.product), selectinload(OrderBookOrder.delivery_point))
        .where(OrderBookOrder.id == order_id)
        .with_for_update()
        .execution_options(populate_existing=True)
    )
    order = locked_result.scalars().first()
    if order is None:
        raise HTTPException(status_code=404, detail="Order not found")
    if (
        order.product_id != locked_product_id
        or order.delivery_point_id != locked_delivery_point_id
        or order.availability_window != locked_window
    ):
        await db.rollback()
        raise HTTPException(status_code=409, detail="Order slice changed; retry the cancellation")
    if order.creation_method == OrderCreationMethod.MARKET_SUPPORT:
        require_matching_etag(if_match, order_id=order.id, version=order.version)
        await lock_support_order_parties(db, order)
    else:
        await lock_and_load_market_organizations(
            db,
            [effective_organization_id],
            actor_ownerships=(
                MarketActorOwnership(current_user.id, effective_organization_id),
            ),
        )

    before_state = await _watchlist_before_state(db, order)
    benchmark_key: LiveBenchmarkKey | None = (
        order.side,
        order.market_product,
        order.delivery_point_id,
        order.availability_window,
    )
    if order.inventory_item_id is not None and order.remaining_quantity_mt > 0:
        await release_inventory(db, order.inventory_item_id, order.remaining_quantity_mt)
    order.status = OrderBookStatus.CANCELLED
    order.bump_version()
    await rebuild_live_slice_benchmarks_for_keys(db, [benchmark_key])
    await emit_order_updated(db, before=before_state, order=order)
    await record_audit(
        db,
        user_id=current_user.id,
        action=ORDER_CANCELLED,
        resource_type="order",
        resource_id=order.id,
        changes={
            "status": OrderBookStatus.CANCELLED.value,
            "reason": reason,
            "actor_user_id": str(party.actor.id) if party else str(current_user.id),
            "effective_organization_id": str(effective_organization_id),
            "original_principal_user_id": str(order.owner_user_id) if order.owner_user_id else None,
            "cancellation_principal_user_id": str(party.accountable_principal.id) if party else str(current_user.id),
            "support_context_id": str(party.support_context_id) if party else None,
            "original_support_context_id": str(authorization_context) if party and authorization_context else None,
            "version": order.version,
        },
        **request_audit_context(request),
    )
    await commit_market_events(
        db,
        [
            participant_market_event(
                event_type="order_cancelled",
                aggregate_type="order",
                aggregate_id=order.id,
                participant_org_ids=(order.organization_id,),
                payload={
                    **order_activity_provenance(order),
                    "id": str(order.id),
                    "side": order.side.value,
                    "product_name": order.product_name,
                    "fuel_type": order.fuel_type,
                    "region": order.region,
                },
            )
        ],
    )

    return None
