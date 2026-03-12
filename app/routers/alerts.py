"""Price alert CRUD endpoints."""
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select, func
from uuid import UUID

from app.database import get_db
from app.routers.auth_simple import get_current_user
from app.models.user import User
from app.models.alerts import PriceAlert
from app.schemas.alerts import AlertCreate, AlertResponse

router = APIRouter(prefix="/alerts", tags=["alerts"])

FREE_TIER_ALERT_LIMIT = 5


async def create_alert(
    alert_data: AlertCreate,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
) -> AlertResponse:
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    # Enforce free-tier limit: count active alerts for this org
    count_result = await db.execute(
        select(func.count(PriceAlert.id)).where(
            PriceAlert.org_id == current_user.organization_id,
            PriceAlert.is_active == True,
        )
    )
    active_count = count_result.scalar()
    if active_count >= FREE_TIER_ALERT_LIMIT:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"Free tier limit of {FREE_TIER_ALERT_LIMIT} active alerts reached. "
                   "Delete an existing alert to create a new one.",
        )

    alert = PriceAlert(
        org_id=current_user.organization_id,
        product_id=alert_data.product_id,
        delivery_point_id=alert_data.delivery_point_id,
        direction=alert_data.direction,
        threshold_usd=alert_data.threshold_usd,
    )
    db.add(alert)
    await db.commit()
    await db.refresh(alert)
    return alert


async def list_alerts(
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
) -> list[AlertResponse]:
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    result = await db.execute(
        select(PriceAlert)
        .where(PriceAlert.org_id == current_user.organization_id)
        .order_by(PriceAlert.created_at.desc())
    )
    return result.scalars().all()


async def delete_alert(
    alert_id: UUID,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
) -> None:
    if not current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="User must belong to an organization",
        )

    result = await db.execute(
        select(PriceAlert).where(PriceAlert.id == alert_id)
    )
    alert = result.scalars().first()

    if not alert:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Alert not found",
        )

    if alert.org_id != current_user.organization_id:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="You can only delete your own alerts",
        )

    await db.delete(alert)
    await db.commit()


# Register routes on the router
router.post("", response_model=AlertResponse, status_code=status.HTTP_201_CREATED)(create_alert)
router.get("", response_model=list[AlertResponse])(list_alerts)
router.delete("/{alert_id}", status_code=status.HTTP_204_NO_CONTENT)(delete_alert)
