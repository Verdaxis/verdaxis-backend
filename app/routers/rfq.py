from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, or_
from sqlalchemy.orm import selectinload
from uuid import UUID
from datetime import datetime
from decimal import Decimal

from app.database import get_db
from app.models.rfq import PublicListing, RFQMatch, Commission, ListingStatus, MatchStatus, CommissionStatus
from app.models.user import User, Organization, UserRole
from app.schemas.rfq import (
    RFQRequestCreate,
    RFQMatchResponse,
    RFQMatchDetailResponse,
    RFQMatchUpdate,
    RFQMatchComplete,
    CommissionResponse,
    CommissionSummary,
    CommissionUpdate,
)
from app.routers.auth import get_current_user

router = APIRouter(prefix="/rfq", tags=["rfq"])


@router.post("/request", response_model=RFQMatchResponse, status_code=status.HTTP_201_CREATED)
async def create_rfq_request(
    request_data: RFQRequestCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Buyer requests a quote on an anonymized listing.
    """
    if current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can request quotes"
        )
    
    if not request_data.accepted_terms:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="You must accept the terms to proceed"
        )
    
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Buyer must belong to an organization"
        )
    
    # Find the listing
    result = await db.execute(select(PublicListing).where(
        PublicListing.id == request_data.listing_id,
        PublicListing.status == ListingStatus.ACTIVE
    ))
    listing = result.scalars().first()
    
    if not listing:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Listing not found or no longer active"
        )
    
    # Check if buyer already has a pending match for this listing
    result = await db.execute(select(RFQMatch).where(
        RFQMatch.listing_id == listing.id,
        RFQMatch.buyer_id == current_user.organization_id,
        RFQMatch.status == MatchStatus.PENDING
    ))
    existing_match = result.scalars().first()
    
    if existing_match:
        raise HTTPException(
            status_code=status.HTTP_409_CONFLICT,
            detail="You already have a pending RFQ for this listing"
        )
    
    # Create the RFQ match
    rfq_match = RFQMatch(
        listing_id=listing.id,
        buyer_id=current_user.organization_id,
        status=MatchStatus.PENDING,
        buyer_accepted_terms_at=datetime.utcnow(),
    )
    
    db.add(rfq_match)
    await db.commit()
    await db.refresh(rfq_match)
    
    return rfq_match


@router.get("/my-requests", response_model=list[RFQMatchDetailResponse])
async def list_buyer_rfq_requests(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all RFQ requests made by the current buyer.
    """
    if current_user.role != UserRole.BUYER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only buyers can view their RFQ requests"
        )
    
    # Eager load listing to access region/fuel_type
    query = select(RFQMatch).options(selectinload(RFQMatch.listing)).where(
        RFQMatch.buyer_id == current_user.organization_id
    ).order_by(RFQMatch.created_at.desc())
    
    result = await db.execute(query)
    matches = result.scalars().all()
    
    result_list = []
    for match in matches:
        listing = match.listing
        
        # Async query for supplier and buyer orgs
        res_supplier = await db.execute(select(Organization).where(Organization.id == listing.supplier_id))
        supplier = res_supplier.scalars().first()
        
        res_buyer = await db.execute(select(Organization).where(Organization.id == match.buyer_id))
        buyer = res_buyer.scalars().first()
        
        result_list.append(RFQMatchDetailResponse(
            id=match.id,
            listing_id=match.listing_id,
            buyer_id=match.buyer_id,
            status=match.status,
            buyer_accepted_terms_at=match.buyer_accepted_terms_at,
            created_at=match.created_at,
            region=listing.region,
            fuel_type=listing.fuel_type,
            fuel_grade=listing.fuel_grade,
            quantity_mt=listing.quantity_mt,
            price_per_mt_usd=listing.price_per_mt_usd,
            supplier_id=listing.supplier_id,
            supplier_name=supplier.name if supplier else "Unknown",
            buyer_name=buyer.name if buyer else "Unknown",
            final_quantity_mt=match.final_quantity_mt,
            final_price_per_mt=match.final_price_per_mt,
            final_total_usd=match.final_total_usd,
        ))
    
    return result_list


@router.get("/incoming", response_model=list[RFQMatchDetailResponse])
async def list_supplier_incoming_rfqs(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get all incoming RFQ requests for the current supplier.
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can view incoming RFQs"
        )
    
    # Get all listings owned by this supplier
    subquery = select(PublicListing.id).where(
        PublicListing.supplier_id == current_user.organization_id
    )
    
    # Get all matches for those listings, eager loading listing
    query = select(RFQMatch).options(selectinload(RFQMatch.listing)).where(
        RFQMatch.listing_id.in_(subquery)
    ).order_by(RFQMatch.created_at.desc())
    
    result = await db.execute(query)
    matches = result.scalars().all()
    
    result_list = []
    for match in matches:
        listing = match.listing
        
        # Async query for orgs
        res_supplier = await db.execute(select(Organization).where(Organization.id == listing.supplier_id))
        supplier = res_supplier.scalars().first()
        
        res_buyer = await db.execute(select(Organization).where(Organization.id == match.buyer_id))
        buyer = res_buyer.scalars().first()
        
        result_list.append(RFQMatchDetailResponse(
            id=match.id,
            listing_id=match.listing_id,
            buyer_id=match.buyer_id,
            status=match.status,
            buyer_accepted_terms_at=match.buyer_accepted_terms_at,
            created_at=match.created_at,
            region=listing.region,
            fuel_type=listing.fuel_type,
            fuel_grade=listing.fuel_grade,
            quantity_mt=listing.quantity_mt,
            price_per_mt_usd=listing.price_per_mt_usd,
            supplier_id=listing.supplier_id,
            supplier_name=supplier.name if supplier else "Unknown",
            buyer_name=buyer.name if buyer else "Unknown",
            final_quantity_mt=match.final_quantity_mt,
            final_price_per_mt=match.final_price_per_mt,
            final_total_usd=match.final_total_usd,
        ))
    
    return result_list


@router.put("/{match_id}/respond", response_model=RFQMatchResponse)
async def respond_to_rfq(
    match_id: UUID,
    response_data: RFQMatchUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Supplier responds to an RFQ (accept or decline).
    """
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only suppliers can respond to RFQs"
        )
    
    # Eager load listing to check ownership
    result = await db.execute(select(RFQMatch).options(selectinload(RFQMatch.listing)).where(RFQMatch.id == match_id))
    match = result.scalars().first()
    
    if not match:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="RFQ match not found"
        )
    
    # Verify supplier owns the listing
    if match.listing.supplier_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only respond to RFQs for your own listings"
        )
    
    if match.status != MatchStatus.PENDING:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"Cannot respond to RFQ with status: {match.status}"
        )
    
    match.status = response_data.status
    match.supplier_responded_at = datetime.utcnow()
    
    await db.commit()
    await db.refresh(match)
    
    return match


@router.put("/{match_id}/complete", response_model=RFQMatchResponse)
async def complete_rfq(
    match_id: UUID,
    completion_data: RFQMatchComplete,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Complete an RFQ match and calculate commission.
    """
    # Eager load listing to check ownership
    result = await db.execute(select(RFQMatch).options(selectinload(RFQMatch.listing)).where(RFQMatch.id == match_id))
    match = result.scalars().first()
    
    if not match:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="RFQ match not found"
        )
    
    # Can be completed by buyer or supplier
    is_buyer = match.buyer_id == current_user.organization_id
    is_supplier = match.listing.supplier_id == current_user.organization_id
    
    if not (is_buyer or is_supplier):
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only matched parties can complete this RFQ"
        )
    
    if match.status != MatchStatus.ACCEPTED:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="RFQ must be ACCEPTED before completion"
        )
    
    # Set final deal details
    match.final_quantity_mt = completion_data.final_quantity_mt
    match.final_price_per_mt = completion_data.final_price_per_mt
    match.final_total_usd = completion_data.final_quantity_mt * completion_data.final_price_per_mt
    match.completed_at = datetime.utcnow()
    match.status = MatchStatus.COMPLETED
    
    # Calculate commission
    commission_amount = match.final_total_usd * (match.commission_rate_pct / 100)
    match.commission_amount_usd = commission_amount
    
    # Create commission record
    commission = Commission(
        match_id=match.id,
        amount_usd=commission_amount,
        status=CommissionStatus.PENDING,
    )
    
    db.add(commission)
    await db.commit()
    await db.refresh(match)
    
    return match


# ============== Admin Commission Endpoints ==============

@router.get("/admin/commissions", response_model=list[CommissionResponse])
async def list_all_commissions(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    List all commissions (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    result = await db.execute(select(Commission).order_by(Commission.created_at.desc()))
    commissions = result.scalars().all()
    return commissions


@router.get("/admin/commissions/summary", response_model=CommissionSummary)
async def get_commission_summary(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Get commission summary stats (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    from sqlalchemy import func
    
    query_pending = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.PENDING)
    res_pending = await db.execute(query_pending)
    pending = res_pending.one()
    
    query_invoiced = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.INVOICED)
    res_invoiced = await db.execute(query_invoiced)
    invoiced = res_invoiced.one()
    
    query_paid = select(
        func.count(Commission.id),
        func.coalesce(func.sum(Commission.amount_usd), 0)
    ).where(Commission.status == CommissionStatus.PAID)
    res_paid = await db.execute(query_paid)
    paid = res_paid.one()
    
    return CommissionSummary(
        pending_count=pending[0],
        total_pending_usd=pending[1],
        invoiced_count=invoiced[0],
        total_invoiced_usd=invoiced[1],
        paid_count=paid[0],
        total_paid_usd=paid[1],
    )


@router.put("/admin/commissions/{commission_id}", response_model=CommissionResponse)
async def update_commission(
    commission_id: UUID,
    update_data: CommissionUpdate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    """
    Update commission status (Admin only).
    """
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin access required"
        )
    
    result = await db.execute(select(Commission).where(Commission.id == commission_id))
    commission = result.scalars().first()
    
    if not commission:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Commission not found"
        )
    
    update_dict = update_data.model_dump(exclude_unset=True)
    for field, value in update_dict.items():
        setattr(commission, field, value)
    
    await db.commit()
    await db.refresh(commission)
    
    return commission
