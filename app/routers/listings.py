from fastapi import APIRouter, Depends, HTTPException, status, Query
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func
from typing import Optional
from uuid import UUID

from app.database import get_db
from app.database import get_db
from app.models.orders import PublicListing, ListingStatus
from app.models.user import User, Organization, UserRole
from app.schemas.orders import (
    PublicListingCreate,
    PublicListingUpdate,
    PublicListingResponse,
    PublicListingSupplierResponse,
    AvailabilityWindow,
    FuelGrade,
    AggregatedListingResponse,
)
from app.routers.auth_simple import get_current_user

router = APIRouter(prefix="/listings", tags=["listings"])


@router.get("", response_model=list[PublicListingResponse])
async def list_public_listings(
    region: Optional[str] = Query(None, description="Filter by region"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    availability: Optional[str] = Query(None, description="Filter by availability window"),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all active public listings (anonymized).
    """
    query = select(PublicListing).where(PublicListing.status == ListingStatus.ACTIVE)
    
    if region:
        query = query.where(PublicListing.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.where(PublicListing.fuel_type.ilike(f"%{fuel_type}%"))
    if availability:
        query = query.where(PublicListing.availability_window == availability)
    
    query = query.order_by(PublicListing.created_at.desc())
    result = await db.execute(query)
    listings = result.scalars().all()
    return listings


@router.get("/aggregated", response_model=list[AggregatedListingResponse])
async def list_aggregated_listings(
    db: AsyncSession = Depends(get_db),
):
    """
    Get listings aggregated by region and fuel type.
    """
    query = (
        select(
            PublicListing.region,
            PublicListing.fuel_type,
            func.min(PublicListing.price_per_mt_usd).label("min_price"),
            func.max(PublicListing.price_per_mt_usd).label("max_price"),
            func.sum(PublicListing.quantity_mt).label("total_quantity"),
            func.count(PublicListing.id).label("listing_count"),
        )
        .where(PublicListing.status == ListingStatus.ACTIVE)
        .group_by(PublicListing.region, PublicListing.fuel_type)
        .order_by(PublicListing.region, PublicListing.fuel_type)
    )
    
    result = await db.execute(query)
    # Result rows are keyed by column name or index
    rows = result.all()
    
    aggregated_data = []
    for row in rows:
        aggregated_data.append(AggregatedListingResponse(
            region=row.region,
            fuel_type=row.fuel_type,
            min_price=row.min_price,
            max_price=row.max_price,
            total_quantity=row.total_quantity,
            listing_count=row.listing_count
        ))
        
    return aggregated_data


@router.get("/my", response_model=list[PublicListingSupplierResponse])
async def list_my_listings(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all listings created by the current supplier.
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can view their listings"
        )
    
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Supplier must belong to an organization"
        )
    
    # Eager load 'matches' to avoid N+1 or async error when calculating match_count
    from sqlalchemy.orm import selectinload
    query = (
        select(PublicListing)
        .options(selectinload(PublicListing.orders))
        .where(PublicListing.supplier_id == current_user.organization_id)
        .order_by(PublicListing.created_at.desc())
    )
    
    result = await db.execute(query)
    listings = result.scalars().all()
    
    result_list = []
    for listing in listings:
        # Use from_orm (model_validate) and then inject match_count
        item = PublicListingSupplierResponse.model_validate(listing)
        item.match_count = len(listing.orders)
        result_list.append(item)
    
    return result_list


@router.post("", response_model=PublicListingResponse, status_code=status.HTTP_201_CREATED)
async def create_listing(
    listing_data: PublicListingCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Create a new public listing.
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can create listings"
        )
    
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Supplier must belong to an organization"
        )
    
    new_listing = PublicListing(
        supplier_id=current_user.organization_id,
        region=listing_data.region,
        fuel_type=listing_data.fuel_type,
        fuel_grade=listing_data.fuel_grade,
        quantity_mt=listing_data.quantity_mt,
        price_per_mt_usd=listing_data.price_per_mt_usd,
        availability_window=listing_data.availability_window,
        tier_label=listing_data.tier_label,
        certifications=listing_data.certifications,
    )
    
    db.add(new_listing)
    await db.commit()
    await db.refresh(new_listing)
    
    return new_listing


@router.get("/{listing_id}", response_model=PublicListingResponse)
async def get_listing(
    listing_id: UUID,
    db: AsyncSession = Depends(get_db),
):
    """
    Get a single listing by ID (anonymized).
    """
    result = await db.execute(select(PublicListing).where(PublicListing.id == listing_id))
    listing = result.scalars().first()
    
    if not listing:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Listing not found"
        )
    
    return listing


@router.put("/{listing_id}", response_model=PublicListingResponse)
async def update_listing(
    listing_id: UUID,
    update_data: PublicListingUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Update a listing.
    """
    result = await db.execute(select(PublicListing).where(PublicListing.id == listing_id))
    listing = result.scalars().first()
    
    if not listing:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Listing not found"
        )
    
    if listing.supplier_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only update your own listings"
        )
    
    # Update fields
    update_dict = update_data.model_dump(exclude_unset=True)
    for field, value in update_dict.items():
        setattr(listing, field, value)
    
    await db.commit()
    await db.refresh(listing)
    
    return listing


@router.delete("/{listing_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_listing(
    listing_id: UUID,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Delete (deactivate) a listing.
    """
    result = await db.execute(select(PublicListing).where(PublicListing.id == listing_id))
    listing = result.scalars().first()
    
    if not listing:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Listing not found"
        )
    
    if listing.supplier_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only delete your own listings"
        )
    
    # Soft delete by setting status to INACTIVE
    listing.status = ListingStatus.INACTIVE
    await db.commit()
    
    return None


@router.get("/regions/list", response_model=list[str])
async def list_regions(db: AsyncSession = Depends(get_db)):
    """
    Get list of unique regions from active listings.
    """
    query = select(PublicListing.region).where(
        PublicListing.status == ListingStatus.ACTIVE
    ).distinct()
    
    result = await db.execute(query)
    regions = result.scalars().all()
    
    return list(regions)


@router.get("/fuel-types/list", response_model=list[str])
async def list_fuel_types(db: AsyncSession = Depends(get_db)):
    """
    Get list of unique fuel types from active listings.
    """
    query = select(PublicListing.fuel_type).where(
        PublicListing.status == ListingStatus.ACTIVE
    ).distinct()
    
    result = await db.execute(query)
    fuel_types = result.scalars().all()
    
    return list(fuel_types)
