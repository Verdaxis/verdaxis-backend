from fastapi import APIRouter, Depends, HTTPException, status, Query
from sqlalchemy.orm import Session
from sqlalchemy import func
from typing import Optional
from uuid import UUID

from app.database import get_db
from app.models.rfq import PublicListing, ListingStatus
from app.models.user import User, Organization, UserRole
from app.schemas.rfq import (
    PublicListingCreate,
    PublicListingUpdate,
    PublicListingResponse,
    PublicListingSupplierResponse,
    AvailabilityWindow,
    FuelGrade,
)
from app.routers.auth import get_current_user

router = APIRouter(prefix="/api/listings", tags=["listings"])


@router.get("", response_model=list[PublicListingResponse])
async def list_public_listings(
    region: Optional[str] = Query(None, description="Filter by region"),
    fuel_type: Optional[str] = Query(None, description="Filter by fuel type"),
    availability: Optional[str] = Query(None, description="Filter by availability window"),
    db: Session = Depends(get_db),
):
    """
    Get all active public listings (anonymized).
    This endpoint is accessible to all authenticated users.
    Supplier identity is hidden.
    """
    query = db.query(PublicListing).filter(PublicListing.status == ListingStatus.ACTIVE)
    
    if region:
        query = query.filter(PublicListing.region.ilike(f"%{region}%"))
    if fuel_type:
        query = query.filter(PublicListing.fuel_type.ilike(f"%{fuel_type}%"))
    if availability:
        query = query.filter(PublicListing.availability_window == availability)
    
    listings = query.order_by(PublicListing.created_at.desc()).all()
    return listings


@router.get("/my", response_model=list[PublicListingSupplierResponse])
async def list_my_listings(
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
):
    """
    Get all listings created by the current supplier.
    Only accessible to SUPPLIER role.
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
    
    listings = db.query(PublicListing).filter(
        PublicListing.supplier_id == current_user.organization_id
    ).order_by(PublicListing.created_at.desc()).all()
    
    # Add match count for each listing
    result = []
    for listing in listings:
        listing_dict = PublicListingSupplierResponse.model_validate(listing).model_dump()
        listing_dict["match_count"] = len(listing.matches) if listing.matches else 0
        result.append(PublicListingSupplierResponse(**listing_dict))
    
    return result


@router.post("", response_model=PublicListingResponse, status_code=status.HTTP_201_CREATED)
async def create_listing(
    listing_data: PublicListingCreate,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
):
    """
    Create a new public listing.
    Only accessible to SUPPLIER role.
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
    db.commit()
    db.refresh(new_listing)
    
    return new_listing


@router.get("/{listing_id}", response_model=PublicListingResponse)
async def get_listing(
    listing_id: UUID,
    db: Session = Depends(get_db),
):
    """
    Get a single listing by ID (anonymized).
    """
    listing = db.query(PublicListing).filter(PublicListing.id == listing_id).first()
    
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
    db: Session = Depends(get_db),
):
    """
    Update a listing.
    Only the supplier who created it can update.
    """
    listing = db.query(PublicListing).filter(PublicListing.id == listing_id).first()
    
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
    
    db.commit()
    db.refresh(listing)
    
    return listing


@router.delete("/{listing_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_listing(
    listing_id: UUID,
    current_user: User = Depends(get_current_user),
    db: Session = Depends(get_db),
):
    """
    Delete (deactivate) a listing.
    Only the supplier who created it can delete.
    """
    listing = db.query(PublicListing).filter(PublicListing.id == listing_id).first()
    
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
    db.commit()
    
    return None


@router.get("/regions/list", response_model=list[str])
async def list_regions(db: Session = Depends(get_db)):
    """
    Get list of unique regions from active listings.
    """
    regions = db.query(PublicListing.region).filter(
        PublicListing.status == ListingStatus.ACTIVE
    ).distinct().all()
    
    return [r[0] for r in regions]


@router.get("/fuel-types/list", response_model=list[str])
async def list_fuel_types(db: Session = Depends(get_db)):
    """
    Get list of unique fuel types from active listings.
    """
    fuel_types = db.query(PublicListing.fuel_type).filter(
        PublicListing.status == ListingStatus.ACTIVE
    ).distinct().all()
    
    return [f[0] for f in fuel_types]
