"""Surveillance API endpoints — STORY S6-005.

Access is restricted to COMPLIANCE_OFFICER and ADMIN roles.
"""
from typing import Optional
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.surveillance import SurveillanceEvent, SurveillanceStatus
from app.models.user import User, UserRole
from app.routers.auth_simple import get_current_user
from app.schemas.surveillance import SurveillanceEventResponse, SurveillanceEventUpdate

router = APIRouter(prefix="/surveillance", tags=["surveillance"])

_ALLOWED_ROLES = {UserRole.COMPLIANCE_OFFICER, UserRole.ADMIN}


def _require_surveillance_access(current_user: User = Depends(get_current_user)) -> User:
    if current_user.role not in _ALLOWED_ROLES:
        raise HTTPException(
            status_code=403,
            detail="Surveillance access requires COMPLIANCE_OFFICER or ADMIN role",
        )
    return current_user


@router.get("/events", response_model=list[SurveillanceEventResponse])
async def list_events(
    type: Optional[str] = Query(None, description="Filter by surveillance type"),
    status: Optional[str] = Query(None, description="Filter by status"),
    severity: Optional[str] = Query(None, description="Filter by severity"),
    db: AsyncSession = Depends(get_db),
    _: User = Depends(_require_surveillance_access),
):
    """List surveillance events with optional filters.

    Only COMPLIANCE_OFFICER and ADMIN may access this endpoint.
    """
    stmt = select(SurveillanceEvent)

    if type is not None:
        stmt = stmt.where(SurveillanceEvent.type == type)
    if status is not None:
        stmt = stmt.where(SurveillanceEvent.status == status)
    if severity is not None:
        stmt = stmt.where(SurveillanceEvent.severity == severity)

    stmt = stmt.order_by(SurveillanceEvent.created_at.desc())

    result = await db.execute(stmt)
    return result.scalars().all()


@router.patch("/events/{event_id}", response_model=SurveillanceEventResponse)
async def update_event(
    event_id: UUID,
    update: SurveillanceEventUpdate,
    db: AsyncSession = Depends(get_db),
    _: User = Depends(_require_surveillance_access),
):
    """Update status and/or notes on a surveillance event.

    Only COMPLIANCE_OFFICER and ADMIN may update events.
    """
    result = await db.execute(
        select(SurveillanceEvent).where(SurveillanceEvent.id == event_id)
    )
    event = result.scalar_one_or_none()

    if event is None:
        raise HTTPException(status_code=404, detail="Surveillance event not found")

    if update.status is not None:
        event.status = SurveillanceStatus(update.status.value)
    if update.notes is not None:
        event.notes = update.notes

    await db.commit()
    await db.refresh(event)
    return event
