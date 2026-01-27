from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.marketplace import QuoteRequest, QuoteStatus
from app.schemas.marketplace import QuoteCreate, QuoteUpdate, QuoteResponse
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
    stmt = select(QuoteRequest)
    
    # RBAC Filtering
    if current_user.role == UserRole.BUYER:
        stmt = stmt.where(QuoteRequest.buyer_id == current_user.organization_id)
    elif current_user.role == UserRole.SUPPLIER:
        # Suppliers see requests in pending/open state for ports they serve? 
        # Or specifically assigned. Simplified: Access to all for demo unless awarded
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
    if current_user.role != UserRole.BUYER:
         raise HTTPException(status_code=403, detail="Only buyers can create RFQs")

    db_quote = QuoteRequest(
        **quote.model_dump(),
        buyer_id=current_user.organization_id,
        status=QuoteStatus.Pending
    )
    
    db.add(db_quote)
    await db.commit()
    await db.refresh(db_quote)
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
