from decimal import Decimal
from typing import Any, Annotated, List
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy import func, or_
from sqlalchemy.exc import DBAPIError, IntegrityError
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.catalog import DeliveryPoint, Product
from app.models.marketplace import InventoryItem, FuelType as ModelFuelType
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderSide,
    Trade,
)
from app.schemas.marketplace import InventoryCreate, InventoryItemUpdate, InventoryResponse
from app.models.user import Organization, User, UserRole
from app.routers.auth_simple import get_current_user
from app.middleware.execution import require_execution_eligible_user
from app.services.availability_windows import SPOT_WINDOW, availability_window_expiry
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import INVENTORY_PUBLISHED
from app.services.provenance import snapshot_organization_provenance
from app.services.market_provenance import order_market_provenance
from app.services.market_data_eligibility import (
    active_market_catalog_clauses,
    canonical_delivery_point_clause,
    canonical_market_product_expression,
    current_public_order_clause,
    public_order_collection_provenance_clause,
)
from app.services.execution_policy import (
    execution_party_is_eligible,
    order_is_execution_qualified,
)
from app.services.market_locks import acquire_market_slice_lock
from app.services.inventory_reservations import assert_inventory_mutable, reserve_inventory
from app.services.idempotency import (
    INVENTORY_PUBLISH_OPERATION,
    acquire_idempotency_lock,
    idempotency_request_hash,
)
from app.services.order_lifecycle import expire_market_slice_orders
from app.services.auto_match_side_effects import collect_auto_match_side_effects
from app.services.activity import order_activity_provenance
from app.services.behavioral_analytics import (
    order_created_event,
    track_analytics_event,
    trade_created_event,
)
from app.services.live_benchmarks import rebuild_live_slice_benchmarks_for_keys
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.market_transactions import (
    is_retryable_market_transaction_error,
    retry_market_transaction,
)
from app.services import market_transactions
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)
from app.services.watchlist_events import _best_slice_price, emit_order_created
import logging

logger = logging.getLogger(__name__)

router = APIRouter()

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


async def _resolve_catalog_product(db: AsyncSession, item: InventoryItem) -> Product | None:
    """Resolve only an exact active canonical catalog identity."""
    product_name = (item.product_name or "").strip()
    if not product_name:
        return None
    stmt = select(Product).where(
        Product.is_active.is_(True),
        Product.name == product_name,
        canonical_market_product_expression(Product).is_not(None),
    )
    result = await db.execute(stmt)
    return result.scalar_one_or_none()


async def _resolve_delivery_point(db: AsyncSession, item: InventoryItem) -> DeliveryPoint | None:
    """Map an inventory item port to a delivery point when the names line up."""
    port = getattr(item, "port", None)
    port_name = getattr(port, "name", None)
    if not port_name:
        return None

    stmt = select(DeliveryPoint).where(
        canonical_delivery_point_clause(DeliveryPoint),
        DeliveryPoint.name == port_name.strip(),
    )
    result = await db.execute(stmt)
    return result.scalar_one_or_none()


@router.get("/inventory", response_model=List[InventoryResponse])
async def list_inventory(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
):
    if current_user.role != UserRole.SUPPLIER:
        # Buyers might see aggregated inventory, but for now strict scoping
        raise HTTPException(status_code=403, detail="Forbidden")

    stmt = (
        select(InventoryItem)
        .where(InventoryItem.supplier_id == current_user.organization_id)
        .order_by(InventoryItem.updated_at.desc(), InventoryItem.id)
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    items = result.scalars().all()
    return items

@router.post("/inventory", response_model=InventoryResponse)
@retry_market_transaction()
async def add_inventory(
    item: InventoryCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization. Please complete onboarding first.")

    await lock_and_load_market_organizations(
        db,
        [current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    try:
        # Convert schema data to plain dict with string enum values
        # to avoid cross-module enum class mismatches
        item_data = item.model_dump()
        item_data['fuel_type'] = ModelFuelType(item.fuel_type.value)

        db_item = InventoryItem(
            **item_data,
            supplier_id=current_user.organization_id,
            owner_user_id=current_user.id,
        )

        db.add(db_item)
        await db.commit()
        await db.refresh(db_item)
        return db_item
    except HTTPException:
        raise
    except DBAPIError as exc:
        if is_retryable_market_transaction_error(exc):
            raise
        await db.rollback()
        logger.exception(
            "inventory_create_failed",
            extra={"organization_id": str(current_user.organization_id)},
        )
        raise HTTPException(status_code=500, detail="Unable to create inventory item")
    except Exception:
        await db.rollback()
        logger.exception("inventory_create_failed", extra={"organization_id": str(current_user.organization_id)})
        raise HTTPException(status_code=500, detail="Unable to create inventory item")

@router.patch("/inventory/{item_id}", response_model=InventoryResponse)
@retry_market_transaction()
async def update_inventory(
    item_id: UUID,
    updates: InventoryItemUpdate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization")
    await lock_and_load_market_organizations(
        db,
        [current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    stmt = select(InventoryItem).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id
    ).with_for_update()
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    update_data = updates.model_dump(exclude_unset=True)
    await assert_inventory_mutable(db, item)
    for field, value in update_data.items():
        setattr(item, field, value)

    from datetime import datetime, UTC
    item.updated_at = datetime.now(UTC)

    await db.commit()
    await db.refresh(item)
    return item

@router.delete("/inventory/{item_id}", status_code=204)
@retry_market_transaction()
async def delete_inventory(
    item_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can manage inventory")

    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization")
    await lock_and_load_market_organizations(
        db,
        [current_user.organization_id],
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    stmt = select(InventoryItem).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id
    ).with_for_update()
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    await assert_inventory_mutable(db, item, deleting=True)

    await db.delete(item)
    await db.commit()

@router.post("/inventory/{item_id}/publish")
@retry_market_transaction()
async def publish_inventory_item(
    item_id: UUID,
    request: Request,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_execution_eligible_user)],
):
    """Convert an inventory item into an ASK listing on the unified orderbook."""
    if not current_user.organization_id:
        raise HTTPException(status_code=400, detail="User has no organization")

    idempotency_key = request.headers.get("Idempotency-Key")
    request_hash = None
    if idempotency_key:
        idempotency_key = idempotency_key.strip()
        if not idempotency_key or len(idempotency_key) > 255:
            raise HTTPException(status_code=400, detail="Idempotency-Key must be 1-255 characters")
        request_hash = idempotency_request_hash({"inventory_item_id": str(item_id)})
        await acquire_idempotency_lock(
            db,
            tenant_id=current_user.organization_id,
            operation=INVENTORY_PUBLISH_OPERATION,
            key=idempotency_key,
        )
        replay = (
            await db.execute(
                select(OrderBookOrder).where(
                    OrderBookOrder.organization_id == current_user.organization_id,
                    OrderBookOrder.idempotency_operation == INVENTORY_PUBLISH_OPERATION,
                    OrderBookOrder.idempotency_key == idempotency_key,
                )
            )
        ).scalar_one_or_none()
        if replay is not None:
            if replay.idempotency_request_hash != request_hash:
                raise HTTPException(
                    status_code=409,
                    detail="Idempotency-Key was reused with a different request",
                )
            return {"status": "published", "listing_id": str(replay.id)}

    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can publish inventory")

    # Revalidate the concrete supplier and exact tenant under row locks in the
    # same transaction that publishes the executable order.
    locked_user = (
        await db.execute(select(User).where(User.id == current_user.id).with_for_update())
    ).scalar_one_or_none()
    locked_organization = (
        await db.execute(
            select(Organization).where(Organization.id == current_user.organization_id).with_for_update()
        )
    ).scalar_one_or_none()
    if not await execution_party_is_eligible(
        db,
        user=locked_user,
        organization=locked_organization,
    ) or locked_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Supplier is no longer execution-qualified")

    stmt = select(InventoryItem).options(selectinload(InventoryItem.port)).where(
        InventoryItem.id == item_id,
        InventoryItem.supplier_id == current_user.organization_id,
    ).with_for_update()
    result = await db.execute(stmt)
    item = result.scalar_one_or_none()

    if not item:
        raise HTTPException(status_code=404, detail="Inventory item not found")

    # Replay an existing publication before checking stock. A retry must not
    # turn a successful publication into a false out-of-stock error.
    existing_stmt = (
        select(OrderBookOrder)
        .where(
            OrderBookOrder.inventory_item_id == item.id,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            or_(OrderBookOrder.expires_at.is_(None), OrderBookOrder.expires_at > func.now()),
        )
        .limit(1)
    )
    existing_listing = (await db.execute(existing_stmt)).scalar_one_or_none()
    if existing_listing is not None:
        return {"status": "published", "listing_id": str(existing_listing.id)}

    if item.price_per_mt_usd is None:
        raise HTTPException(status_code=400, detail="Inventory item missing price")

    product = await _resolve_catalog_product(db, item)
    if not product:
        raise HTTPException(status_code=400, detail="Unable to map inventory item to a catalog product")

    delivery_point = await _resolve_delivery_point(db, item)
    if delivery_point is None:
        raise HTTPException(
            status_code=400,
            detail="Inventory port is not an active canonical delivery point",
        )
    await market_transactions.transaction_boundary_hook(
        "before_market_slice_lock",
        operation="order_admission",
        aggregate_id=item.id,
    )
    await acquire_market_slice_lock(
        db,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        availability_window=SPOT_WINDOW,
    )
    _, organizations = await expire_market_slice_orders(
        db,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        availability_window=SPOT_WINDOW,
        additional_organization_ids=(current_user.organization_id,),
        require_approved_organization_ids=(current_user.organization_id,),
        actor_ownerships=(
            MarketActorOwnership(current_user.id, current_user.organization_id),
        ),
    )

    # Re-read the inventory row after the slice lock, then re-check the
    # existing listing and stock under its row lock.
    locked_item_result = await db.execute(
        select(InventoryItem).options(selectinload(InventoryItem.port)).where(
            InventoryItem.id == item_id,
            InventoryItem.supplier_id == current_user.organization_id,
        ).with_for_update().execution_options(populate_existing=True)
    )
    item = locked_item_result.scalar_one_or_none()
    if item is None:
        raise HTTPException(status_code=404, detail="Inventory item not found")
    locked_product = await _resolve_catalog_product(db, item)
    locked_delivery_point = await _resolve_delivery_point(db, item)
    if (
        locked_product is None
        or locked_product.id != product.id
        or locked_delivery_point is None
        or locked_delivery_point.id != delivery_point.id
    ):
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail="Inventory market slice changed; retry publication",
        )
    existing_listing = (await db.execute(existing_stmt.execution_options(populate_existing=True))).scalar_one_or_none()
    if existing_listing is not None:
        return {"status": "published", "listing_id": str(existing_listing.id)}
    expired_backlog = (
        await db.execute(
            select(OrderBookOrder.id)
            .where(
                OrderBookOrder.inventory_item_id == item.id,
                OrderBookOrder.status.in_(
                    [OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]
                ),
                OrderBookOrder.expires_at.is_not(None),
                OrderBookOrder.expires_at <= func.now(),
            )
            .limit(1)
        )
    ).scalar_one_or_none()
    if expired_backlog is not None:
        raise HTTPException(
            status_code=409,
            detail="Inventory listing expiry backlog must be reconciled before publication",
        )
    quantity = Decimal(str(item.current_stock_mt or 0))
    if quantity <= 0:
        raise HTTPException(status_code=400, detail="Inventory quantity must be positive")
    available_stock = Decimal(str(item.current_stock_mt or 0))
    if quantity > available_stock:
        raise HTTPException(status_code=400, detail="Inventory stock is already reserved or unavailable")

    organization = organizations[current_user.organization_id]
    provenance = snapshot_organization_provenance(organization)

    listing = OrderBookOrder(
        organization_id=current_user.organization_id,
        owner_user_id=current_user.id,
        provenance=provenance,
        inventory_item_id=item.id,
        side=OrderSide.ASK,
        product_id=product.id,
        delivery_point_id=delivery_point.id,
        port_id=item.port_id,
        quantity_mt=quantity,
        remaining_quantity_mt=quantity,
        price_per_mt_usd=Decimal(str(item.price_per_mt_usd)),
        availability_window=SPOT_WINDOW,
        certifications=["INVENTORY_CERTIFIED"] if item.is_certified else [],
        status=OrderBookStatus.OPEN,
        idempotency_key=idempotency_key,
        idempotency_operation=(INVENTORY_PUBLISH_OPERATION if idempotency_key else None),
        idempotency_request_hash=request_hash,
        expires_at=(
            availability_window_expiry(SPOT_WINDOW)
            if getattr(provenance, "value", provenance) == "DEMO"
            else None
        ),
        **{field: getattr(item, field) for field in SUPPLIER_METADATA_FIELDS},
    )
    listing.product = product
    listing.delivery_point = delivery_point
    listing.organization = organization
    if not order_is_execution_qualified(listing):
        raise HTTPException(status_code=400, detail="Inventory is not execution-qualified for marketplace publication")

    previous_best_price = await _best_slice_price(
        db,
        market_product_code=listing.market_product,
        delivery_point_id=listing.delivery_point_id,
        availability_window_code=listing.availability_window,
        side=OrderSide.ASK,
    )
    resting_side_previous_best_price = await _best_slice_price(
        db,
        market_product_code=listing.market_product,
        delivery_point_id=listing.delivery_point_id,
        availability_window_code=listing.availability_window,
        side=OrderSide.BID,
    )
    item = await reserve_inventory(db, item.id, quantity)
    db.add(listing)
    try:
        await db.flush()
    except IntegrityError:
        await db.rollback()
        if not idempotency_key:
            raise
        replay = (
            await db.execute(
                select(OrderBookOrder).where(
                    OrderBookOrder.organization_id == current_user.organization_id,
                    OrderBookOrder.idempotency_operation == INVENTORY_PUBLISH_OPERATION,
                    OrderBookOrder.idempotency_key == idempotency_key,
                )
            )
        ).scalar_one_or_none()
        if replay is None:
            raise
        if replay.idempotency_request_hash != request_hash:
            raise HTTPException(
                status_code=409,
                detail="Idempotency-Key was reused with a different request",
            )
        return {"status": "published", "listing_id": str(replay.id)}

    matched_trades = []
    from app.config import settings
    if settings.AUTO_MATCHING_ENABLED:
        from app.services.matching_engine import match_order
        matched_trades = await match_order(db, listing, is_anonymous=True)
    await rebuild_live_slice_benchmarks_for_keys(
        db,
        [
            (OrderSide.ASK, product.market_product, delivery_point.id, SPOT_WINDOW),
            (OrderSide.BID, product.market_product, delivery_point.id, SPOT_WINDOW),
        ],
    )
    await emit_order_created(db, listing, previous_best_price=previous_best_price)
    committed_events = await collect_auto_match_side_effects(
        db,
        triggering_order=listing,
        trades=matched_trades,
        actor_user_id=current_user.id,
        audit_context=request_audit_context(request),
        resting_side_previous_best_price=resting_side_previous_best_price,
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=INVENTORY_PUBLISHED,
        resource_type="inventory",
        resource_id=item.id,
        changes={
            "inventory_item_id": str(item.id),
            "listing_id": str(listing.id),
            "quantity_mt": str(listing.quantity_mt),
            "price_per_mt_usd": str(listing.price_per_mt_usd),
            "status": listing.status.value,
        },
        **request_audit_context(request),
    )
    committed_events.append(
        participant_market_event(
            event_type="order_created",
            aggregate_type="order",
            aggregate_id=listing.id,
            participant_org_ids={
                listing.organization_id,
                *(trade.buyer_id for trade in matched_trades),
                *(trade.seller_id for trade in matched_trades),
            },
            payload={
                **order_activity_provenance(listing),
                "id": str(listing.id),
                "side": listing.side.value,
                "product_name": listing.product_name,
                "fuel_type": listing.fuel_type,
                "region": listing.region,
                "price": str(listing.price_per_mt_usd),
                "quantity": str(listing.remaining_quantity_mt),
            },
        )
    )
    await enqueue_market_events(db, committed_events)
    await db.commit()
    await db.refresh(listing)

    track_analytics_event(
        order_created_event(current_user, listing, request=request),
        request=request,
    )
    for _trade in matched_trades:
        track_analytics_event(
            trade_created_event(current_user, order=listing, request=request),
            request=request,
        )
    return {"status": "published", "listing_id": str(listing.id)}


def _listing_payload(order: OrderBookOrder, match_count: int = 0) -> dict[str, Any]:
    return {
        "id": str(order.id),
        "product_name": order.product_name,
        "market_product": order.market_product,
        "fuel_type": order.fuel_type,
        "fuel_grade": order.fuel_grade,
        "quantity_mt": str(order.quantity_mt),
        "price_per_mt_usd": str(order.price_per_mt_usd),
        "region": order.region,
        "availability_window": order.availability_window,
        "certifications": order.certifications or [],
        "certification_declared": order.certification_declared,
        "certification_scheme": order.certification_scheme,
        "specification_standard": order.specification_standard,
        "msds_available": order.msds_available,
        "carbon_intensity_gco2_mj": str(order.carbon_intensity_gco2_mj) if order.carbon_intensity_gco2_mj is not None else None,
        "carbon_intensity_method": order.carbon_intensity_method,
        "feedstock": order.feedstock,
        "origin": order.origin,
        "off_spec": order.off_spec,
        "off_spec_notes": order.off_spec_notes,
        "status": order.status.value if hasattr(order.status, "value") else str(order.status),
        "match_count": match_count,
        "expires_at": order.expires_at.isoformat() if order.expires_at else None,
        "observed_at": order.created_at.isoformat() if order.created_at else None,
        **order_market_provenance(order),
    }


@router.get("/listings")
async def list_public_listings(
    db: Annotated[AsyncSession, Depends(get_db)],
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
):
    """Bounded compatibility catalog of current canonical ASK listings."""
    stmt = (
        select(OrderBookOrder)
        .join(Product, OrderBookOrder.product_id == Product.id)
        .join(DeliveryPoint, OrderBookOrder.delivery_point_id == DeliveryPoint.id)
        .options(
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(
            OrderBookOrder.side == OrderSide.ASK,
            OrderBookOrder.status.in_([OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED]),
            current_public_order_clause(OrderBookOrder),
            public_order_collection_provenance_clause(OrderBookOrder),
            *active_market_catalog_clauses(Product, DeliveryPoint),
        )
        .order_by(OrderBookOrder.created_at.desc(), OrderBookOrder.id)
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    listings = result.unique().scalars().all()
    return [_listing_payload(order) for order in listings]


@router.get("/listings/my")
async def list_my_listings(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)],
    skip: Annotated[int, Query(ge=0)] = 0,
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
):
    """Supplier's own ASK listings with trade counts."""
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can view own listings")
    if not current_user.organization_id:
        return []

    match_count = (
        select(func.count(Trade.id))
        .where(Trade.ask_order_id == OrderBookOrder.id)
        .correlate(OrderBookOrder)
        .scalar_subquery()
    )
    stmt = (
        select(OrderBookOrder, match_count.label("match_count"))
        .options(
            selectinload(OrderBookOrder.product),
            selectinload(OrderBookOrder.delivery_point),
        )
        .where(
            OrderBookOrder.organization_id == current_user.organization_id,
            OrderBookOrder.side == OrderSide.ASK,
        )
        .order_by(OrderBookOrder.created_at.desc(), OrderBookOrder.id)
        .offset(skip)
        .limit(limit)
    )
    result = await db.execute(stmt)
    return [
        _listing_payload(order, match_count=int(count or 0))
        for order, count in result.unique().all()
    ]
