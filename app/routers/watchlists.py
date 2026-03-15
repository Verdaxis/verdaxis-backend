"""Watchlist CRUD endpoints — saved product preferences per user."""
from typing import Annotated
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy import select, func
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.orm import selectinload

from app.database import get_db
from app.models.user import User
from app.models.watchlist import Watchlist, WatchlistEntry
from app.models.catalog import Product, DeliveryPoint
from app.routers.auth_simple import get_current_user
from app.schemas.watchlist import (
    WatchlistCreateRequest,
    WatchlistEntryAddRequest,
    WatchlistEntryResponse,
    WatchlistResponse,
)

router = APIRouter(prefix="/watchlists", tags=["watchlists"])

MAX_WATCHLISTS_PER_USER = 10
MAX_ENTRIES_PER_WATCHLIST = 50


@router.get("", response_model=list[WatchlistResponse])
async def list_watchlists(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """List all watchlists for the current user with entry counts."""
    stmt = (
        select(Watchlist)
        .where(Watchlist.user_id == current_user.id)
        .options(selectinload(Watchlist.entries))
        .order_by(Watchlist.created_at.desc())
    )
    result = await db.execute(stmt)
    watchlists = result.scalars().all()

    responses = []
    for wl in watchlists:
        # Batch-fetch product and delivery point names for entries
        entry_responses = []
        if wl.entries:
            product_ids = [e.product_id for e in wl.entries]
            dp_ids = [e.delivery_point_id for e in wl.entries if e.delivery_point_id]

            products_map = {}
            if product_ids:
                prod_result = await db.execute(
                    select(Product.id, Product.name).where(Product.id.in_(product_ids))
                )
                products_map = dict(prod_result.all())

            dp_map = {}
            if dp_ids:
                dp_result = await db.execute(
                    select(DeliveryPoint.id, DeliveryPoint.name).where(DeliveryPoint.id.in_(dp_ids))
                )
                dp_map = dict(dp_result.all())

            for entry in wl.entries:
                entry_responses.append(WatchlistEntryResponse(
                    id=entry.id,
                    product_id=entry.product_id,
                    product_name=products_map.get(entry.product_id),
                    delivery_point_id=entry.delivery_point_id,
                    delivery_point_name=dp_map.get(entry.delivery_point_id) if entry.delivery_point_id else None,
                    created_at=entry.created_at,
                ))

        responses.append(WatchlistResponse(
            id=wl.id,
            name=wl.name,
            entry_count=len(wl.entries),
            entries=entry_responses,
            created_at=wl.created_at,
        ))

    return responses


@router.post("", response_model=WatchlistResponse, status_code=201)
async def create_watchlist(
    body: WatchlistCreateRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Create a new watchlist. Max 10 per user."""
    # Check limit
    count_stmt = select(func.count(Watchlist.id)).where(Watchlist.user_id == current_user.id)
    result = await db.execute(count_stmt)
    current_count = result.scalar_one()

    if current_count >= MAX_WATCHLISTS_PER_USER:
        raise HTTPException(
            status_code=400,
            detail=f"Maximum of {MAX_WATCHLISTS_PER_USER} watchlists allowed",
        )

    wl = Watchlist(user_id=current_user.id, name=body.name)
    db.add(wl)
    await db.commit()
    await db.refresh(wl)

    return WatchlistResponse(
        id=wl.id,
        name=wl.name,
        entry_count=0,
        entries=[],
        created_at=wl.created_at,
    )


@router.post("/{watchlist_id}/entries", response_model=WatchlistEntryResponse, status_code=201)
async def add_watchlist_entry(
    watchlist_id: UUID,
    body: WatchlistEntryAddRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Add a product to a watchlist. Max 50 entries per watchlist."""
    # Fetch watchlist, verify ownership
    stmt = select(Watchlist).where(Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id)
    result = await db.execute(stmt)
    wl = result.scalar_one_or_none()
    if not wl:
        raise HTTPException(status_code=404, detail="Watchlist not found")

    # Check entry limit
    entry_count_stmt = select(func.count(WatchlistEntry.id)).where(
        WatchlistEntry.watchlist_id == watchlist_id
    )
    count_result = await db.execute(entry_count_stmt)
    if count_result.scalar_one() >= MAX_ENTRIES_PER_WATCHLIST:
        raise HTTPException(
            status_code=400,
            detail=f"Maximum of {MAX_ENTRIES_PER_WATCHLIST} entries per watchlist",
        )

    # Validate product exists
    product = await db.execute(select(Product).where(Product.id == body.product_id))
    prod = product.scalar_one_or_none()
    if not prod:
        raise HTTPException(status_code=404, detail="Product not found")

    # Validate delivery point if provided
    dp_name = None
    if body.delivery_point_id:
        dp_result = await db.execute(
            select(DeliveryPoint).where(DeliveryPoint.id == body.delivery_point_id)
        )
        dp = dp_result.scalar_one_or_none()
        if not dp:
            raise HTTPException(status_code=404, detail="Delivery point not found")
        dp_name = dp.name

    entry = WatchlistEntry(
        watchlist_id=watchlist_id,
        product_id=body.product_id,
        delivery_point_id=body.delivery_point_id,
    )
    db.add(entry)
    await db.commit()
    await db.refresh(entry)

    return WatchlistEntryResponse(
        id=entry.id,
        product_id=entry.product_id,
        product_name=prod.name,
        delivery_point_id=entry.delivery_point_id,
        delivery_point_name=dp_name,
        created_at=entry.created_at,
    )


@router.delete("/{watchlist_id}/entries/{entry_id}", status_code=204)
async def remove_watchlist_entry(
    watchlist_id: UUID,
    entry_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Remove an entry from a watchlist."""
    # Verify watchlist ownership
    wl_stmt = select(Watchlist.id).where(
        Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id
    )
    wl_result = await db.execute(wl_stmt)
    if not wl_result.scalar_one_or_none():
        raise HTTPException(status_code=404, detail="Watchlist not found")

    # Find and delete entry
    entry_stmt = select(WatchlistEntry).where(
        WatchlistEntry.id == entry_id, WatchlistEntry.watchlist_id == watchlist_id
    )
    entry_result = await db.execute(entry_stmt)
    entry = entry_result.scalar_one_or_none()
    if not entry:
        raise HTTPException(status_code=404, detail="Entry not found")

    await db.delete(entry)
    await db.commit()


@router.delete("/{watchlist_id}", status_code=204)
async def delete_watchlist(
    watchlist_id: UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Delete a watchlist and all its entries."""
    stmt = (
        select(Watchlist)
        .where(Watchlist.id == watchlist_id, Watchlist.user_id == current_user.id)
        .options(selectinload(Watchlist.entries))
    )
    result = await db.execute(stmt)
    wl = result.scalar_one_or_none()
    if not wl:
        raise HTTPException(status_code=404, detail="Watchlist not found")

    await db.delete(wl)
    await db.commit()
