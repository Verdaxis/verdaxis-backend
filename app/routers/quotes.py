from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from sqlalchemy.orm import selectinload
from app.database import get_db
from app.models.marketplace import QuoteRequest, QuoteStatus, QuoteOffer
from app.models.notification import Notification, NotificationType
from app.schemas.marketplace import QuoteCreate, QuoteUpdate, QuoteResponse, QuoteOfferCreate, QuoteOfferResponse
from app.models.user import User, UserRole
from app.core.auth import get_current_user
from typing import List, Annotated
import uuid
from datetime import datetime

router = APIRouter()

@router.get("/quotes", response_model=List[QuoteResponse])
async def list_quotes(
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    stmt = select(QuoteRequest).options(selectinload(QuoteRequest.offers))
    
    # RBAC Filtering
    if current_user.role == UserRole.BUYER:
        stmt = stmt.where(QuoteRequest.buyer_id == current_user.organization_id)
    elif current_user.role == UserRole.SUPPLIER:
        # Suppliers see requests they have offered on OR all pending requests (Marketplace)
        # Simplified for demo: Show all
        pass 
        
    result = await db.execute(stmt)
    quotes = result.scalars().all()
    return quotes

@router.post("/quotes", response_model=QuoteResponse)
async def create_quote(
    quote: QuoteCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    print(f"DEBUG: User: {current_user.email}, Role: {current_user.role} ({type(current_user.role)}), Expected: {UserRole.BUYER} ({type(UserRole.BUYER)})")
    if current_user.role != UserRole.BUYER:
         raise HTTPException(status_code=403, detail=f"Only buyers can create RFQs. You are {current_user.role}")

    db_quote = QuoteRequest(
        **quote.model_dump(),
        buyer_id=current_user.organization_id,
        status=QuoteStatus.Pending
    )
    
    db.add(db_quote)
    await db.commit()
    
    # Reload with relationships to satisfy Pydantic response model
    stmt = select(QuoteRequest).options(selectinload(QuoteRequest.offers)).where(QuoteRequest.id == db_quote.id)
    result = await db.execute(stmt)
    db_quote = result.scalar_one()
    
    return db_quote

@router.patch("/quotes/{quote_id}", response_model=QuoteResponse)
async def update_quote(
    quote_id: str,
    update_data: QuoteUpdate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    stmt = select(QuoteRequest).where(QuoteRequest.id == uuid.UUID(quote_id))
    result = await db.execute(stmt)
    db_quote = result.scalar_one_or_none()
    
    if not db_quote:
        raise HTTPException(status_code=404, detail="Quote not found")
        
    # Apply updates
    update_dict = update_data.model_dump(exclude_unset=True)
    for key, value in update_dict.items():
        setattr(db_quote, key, value)
        
    db_quote.updated_at = datetime.utcnow()
    
    await db.commit()
    await db.refresh(db_quote)
    return db_quote

@router.post("/quotes/{quote_id}/offers", response_model=QuoteOfferResponse)
async def create_offer(
    quote_id: str,
    offer: QuoteOfferCreate,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    if current_user.role != UserRole.SUPPLIER:
        raise HTTPException(status_code=403, detail="Only suppliers can make offers")

    # Verify quote exists
    stmt = select(QuoteRequest).where(QuoteRequest.id == uuid.UUID(quote_id))
    result = await db.execute(stmt)
    quote_request = result.scalar_one_or_none()
    
    if not quote_request:
        raise HTTPException(status_code=404, detail="Quote request not found")

    db_offer = QuoteOffer(
        **offer.model_dump(),
        id=uuid.uuid4(),
        request_id=uuid.UUID(quote_id),
        supplier_id=current_user.organization_id
    )
    
    db.add(db_offer)
    
    # Notify buyer users
    stmt = select(User).where(User.organization_id == quote_request.buyer_id)
    result = await db.execute(stmt)
    buyer_users = result.scalars().all()
    
    for user in buyer_users:
        notification = Notification(
            recipient_id=user.id,
            type=NotificationType.QUOTE_OFFER,
            title="New Quote Offer",
            message=f"A supplier has sent an offer for your {quote_request.fuel_type} quote request.",
            data={"quote_id": str(quote_request.id), "offer_id": str(db_offer.id)}
        )
        db.add(notification)
    
    # Update quote status to Negotiating if still Pending
    if quote_request.status == QuoteStatus.Pending:
        quote_request.status = QuoteStatus.Negotiating
        
    await db.commit()
    await db.refresh(db_offer)
    return db_offer

@router.put("/quotes/{quote_id}/accept/{offer_id}", response_model=QuoteResponse)
async def accept_offer(
    quote_id: str,
    offer_id: str,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(get_current_user)]
):
    # Load quote with offers
    stmt = select(QuoteRequest).options(selectinload(QuoteRequest.offers)).where(QuoteRequest.id == uuid.UUID(quote_id))
    result = await db.execute(stmt)
    quote = result.scalar_one_or_none()
    
    if not quote:
        raise HTTPException(status_code=404, detail="Quote request not found")
        
    if current_user.role != UserRole.BUYER or quote.buyer_id != current_user.organization_id:
        raise HTTPException(status_code=403, detail="Not authorized to accept this offer")

    # Find the offer
    selected_offer = next((o for o in quote.offers if str(o.id) == offer_id), None)
    if not selected_offer:
        raise HTTPException(status_code=404, detail="Offer not found")

    # Update Quote Status
    quote.status = QuoteStatus.Confirmed
    quote.awarded_supplier_id = selected_offer.supplier_id
    quote.final_price_usd = selected_offer.price_per_mt_usd * quote.quantity_mt # Approx total
    quote.final_price_per_mt = selected_offer.price_per_mt_usd
    
    # Mark offer as accepted
    selected_offer.is_accepted = True
    
    # Notify supplier users
    stmt = select(User).where(User.organization_id == selected_offer.supplier_id)
    result = await db.execute(stmt)
    supplier_users = result.scalars().all()
    
    for user in supplier_users:
        notification = Notification(
            recipient_id=user.id,
            type=NotificationType.QUOTE_OFFER,
            title="Offer Accepted",
            message=f"Your offer for {quote.fuel_type} has been accepted!",
            data={"quote_id": str(quote.id), "offer_id": str(selected_offer.id)}
        )
        db.add(notification)
    
    await db.commit()
    await db.commit()
    
    # Reload with relationships
    stmt = select(QuoteRequest).options(selectinload(QuoteRequest.offers)).where(QuoteRequest.id == quote.id)
    result = await db.execute(stmt)
    quote = result.scalar_one()
    
    return quote
