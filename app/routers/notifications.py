from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import desc, select, func, update
from typing import List, Any
import uuid

from app.database import get_db
from app.models.notification import Notification
from app.models.user import User
from app.routers.auth_simple import get_current_user
from pydantic import BaseModel, UUID4
from datetime import datetime

router = APIRouter(
    prefix="/notifications",
    tags=["notifications"],
    responses={404: {"description": "Not found"}},
)

class NotificationResponse(BaseModel):
    id: UUID4
    type: str
    title: str
    message: str
    data: dict | None
    is_read: bool
    created_at: datetime

    class Config:
        from_attributes = True

@router.get("/", response_model=List[NotificationResponse])
async def get_notifications(
    skip: int = 0,
    limit: int = 50,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """
    Get current user's notifications.
    """
    stmt = select(Notification)\
        .filter(Notification.recipient_id == current_user.id)\
        .order_by(desc(Notification.created_at))\
        .offset(skip)\
        .limit(limit)
    
    result = await db.execute(stmt)
    notifications = result.scalars().all()
    return notifications

@router.get("/unread-count")
async def get_unread_count(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """
    Get count of unread notifications.
    """
    stmt = select(func.count())\
        .select_from(Notification)\
        .filter(Notification.recipient_id == current_user.id, Notification.is_read == False)
    
    result = await db.execute(stmt)
    count = result.scalar()
    return {"count": count}

@router.patch("/{notification_id}/read")
async def mark_as_read(
    notification_id: uuid.UUID,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """
    Mark a notification as read.
    """
    stmt = select(Notification).filter(
        Notification.id == notification_id,
        Notification.recipient_id == current_user.id
    )
    result = await db.execute(stmt)
    notification = result.scalars().first()
    
    if not notification:
        raise HTTPException(status_code=404, detail="Notification not found")
        
    notification.is_read = True
    await db.commit()
    await db.refresh(notification)
    return {"status": "success"}

@router.patch("/read-all")
async def mark_all_as_read(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db)
):
    """
    Mark all notifications as read for the current user.
    """
    stmt = update(Notification)\
        .where(Notification.recipient_id == current_user.id, Notification.is_read == False)\
        .values(is_read=True)
    
    await db.execute(stmt)
    await db.commit()
    return {"status": "success"}
