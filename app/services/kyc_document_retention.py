"""Retention checks for account and organization deletion."""
from uuid import UUID

from fastapi import HTTPException
from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.kyc_document import KYCDocument


async def require_no_retained_documents(
    db: AsyncSession, *, user_id: UUID | None = None, organization_id: UUID | None = None,
) -> None:
    predicates = []
    if user_id is not None:
        predicates.append(KYCDocument.user_id == user_id)
    if organization_id is not None:
        predicates.append(KYCDocument.organization_id == organization_id)
    if not predicates:
        return
    retained = await db.scalar(select(KYCDocument.id).where(or_(*predicates)).limit(1))
    if retained is not None:
        raise HTTPException(status_code=409, detail={
            "code": "KYC_DOCUMENTS_RETAINED",
            "message": "Retained KYC documents prevent deletion of this account or organization",
        })
