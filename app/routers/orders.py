from fastapi import APIRouter, Depends, HTTPException, Request, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from uuid import UUID

from app.database import get_db
from app.models.orders import Commission, CommissionStatus
from app.models.user import User, UserRole
from app.schemas.orders import (
    CommissionResponse,
    CommissionSummary,
    CommissionUpdate,
)
from app.routers.auth_simple import get_current_user
from app.schemas.errors import AUTH_RESPONSES
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import COMMISSION_UPDATED

router = APIRouter(
    prefix="/orders",
    tags=["orders"],
    responses=AUTH_RESPONSES,
)


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
    request: Request,
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
    previous_status = commission.status
    for field, value in update_dict.items():
        setattr(commission, field, value)

    changes = {}
    if "status" in update_dict and previous_status != commission.status:
        changes["status"] = {
            "from": previous_status.value,
            "to": commission.status.value,
        }

    await record_audit(
        db,
        user_id=current_user.id,
        action=COMMISSION_UPDATED,
        resource_type="commission",
        resource_id=commission.id,
        changes=changes,
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(commission)

    return commission
