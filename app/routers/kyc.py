"""KYC router — document submission and admin review."""
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, Request, UploadFile, File, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
import uuid
from pydantic import BaseModel

from app.config import settings
from app.database import get_db
from app.models.user import User, UserRole, UserStatus
from app.routers.auth_simple import get_current_user
from app.services.kyc import verify_document_with_gemini
from app.services.email import send_kyc_approved_email, send_kyc_rejected_email
from app.services.audit_service import record_audit, request_audit_context
from app.services.audit_actions import KYC_APPROVED, KYC_REJECTED, KYC_SUBMITTED
from app.services.user_status_transition import record_status_transition

router = APIRouter(prefix="/kyc", tags=["KYC"])


async def read_bounded_upload(upload: UploadFile, *, max_bytes: int) -> bytes:
    """Read an upload without retaining more than its configured bound."""
    declared_size = getattr(upload, "size", None)
    if declared_size is not None and declared_size > max_bytes:
        raise HTTPException(status_code=413, detail="KYC file exceeds the per-file size limit")

    chunks: list[bytes] = []
    total = 0
    chunk_size = 64 * 1024
    while True:
        chunk = await upload.read(min(chunk_size, max_bytes - total + 1))
        if not chunk:
            break
        total += len(chunk)
        if total > max_bytes:
            raise HTTPException(status_code=413, detail="KYC file exceeds the per-file size limit")
        chunks.append(chunk)
    return b"".join(chunks)


async def read_bounded_kyc_documents(
    passport: UploadFile,
    company_doc: UploadFile,
    *,
    max_file_bytes: int,
    max_total_bytes: int,
) -> tuple[bytes, bytes]:
    """Read both KYC documents without exceeding either configured bound."""
    passport_bytes = await read_bounded_upload(passport, max_bytes=max_file_bytes)
    remaining_bytes = max_total_bytes - len(passport_bytes)
    if remaining_bytes <= 0:
        raise HTTPException(status_code=413, detail="KYC upload exceeds the aggregate size limit")
    company_bytes = await read_bounded_upload(
        company_doc, max_bytes=min(max_file_bytes, remaining_bytes)
    )
    return passport_bytes, company_bytes


class AdminRejectBody(BaseModel):
    reason: str


@router.post("/submit")
async def submit_kyc(
    request: Request,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
    passport: UploadFile = File(..., description="Passport or government-issued ID"),
    company_doc: UploadFile = File(..., description="Company registration document"),
):
    """
    Submit KYC documents for advisory Gemini analysis and administrator review.
    Gemini never approves, rejects, or changes the account status.
    """
    passport_bytes, company_bytes = await read_bounded_kyc_documents(
        passport,
        company_doc,
        max_file_bytes=settings.KYC_MAX_FILE_BYTES,
        max_total_bytes=settings.KYC_MAX_TOTAL_BYTES,
    )

    passport_mime = passport.content_type or "image/jpeg"
    company_mime = company_doc.content_type or "image/jpeg"

    passport_result = await verify_document_with_gemini(
        passport_bytes, passport_mime, "passport or government-issued ID"
    )
    company_result = await verify_document_with_gemini(
        company_bytes, company_mime, "company registration document"
    )

    previous_kyc_status = current_user.kyc_status
    current_user.kyc_status = "PENDING"
    current_user.kyc_rejection_reason = None
    await record_audit(
        db,
        user_id=current_user.id,
        action=KYC_SUBMITTED,
        resource_type="kyc",
        resource_id=current_user.id,
        changes={
            "kyc_record_id": str(current_user.id),
            "kyc_status": {"from": previous_kyc_status, "to": "PENDING"},
            "gemini_advisory": {
                "passport_passed": bool(passport_result.get("passed")),
                "company_document_passed": bool(company_result.get("passed")),
            },
        },
        **request_audit_context(request),
    )
    await db.commit()
    return {
        "kyc_status": current_user.kyc_status,
        "message": "KYC submitted for authoritative admin review.",
    }


@router.get("/status")
async def get_kyc_status(
    current_user: Annotated[User, Depends(get_current_user)],
):
    """Return the current user's email verification and KYC status."""
    return {
        "email_verified": current_user.email_verified,
        "kyc_status": current_user.kyc_status,
        "kyc_rejection_reason": current_user.kyc_rejection_reason,
    }


@router.put("/admin/{user_id}/approve")
async def admin_approve_kyc(
    user_id: uuid.UUID,
    request: Request,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Admin: manually approve a user's KYC and activate their account."""
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Admin access required")

    stmt = select(User).where(User.id == user_id)
    result = await db.execute(stmt)
    target = result.scalar_one_or_none()

    if not target:
        raise HTTPException(status_code=404, detail="User not found")

    previous_kyc_status = target.kyc_status
    previous_account_status = target.status
    target.kyc_status = "APPROVED"
    target.kyc_rejection_reason = None
    target.status = UserStatus.APPROVED
    record_status_transition(
        db, target, from_status=previous_account_status, to_status=UserStatus.APPROVED
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=KYC_APPROVED,
        resource_type="kyc",
        resource_id=target.id,
        changes={
            "kyc_record_id": str(target.id),
            "target_user_id": str(target.id),
            "kyc_status": {"from": previous_kyc_status, "to": "APPROVED"},
            "account_status": {"from": previous_account_status.value, "to": UserStatus.APPROVED.value},
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(target)

    await send_kyc_approved_email(target.email, target.first_name or "there")

    return {
        "user_id": str(target.id),
        "kyc_status": target.kyc_status,
        "account_status": target.status.value,
        "message": "KYC approved and account activated.",
    }


@router.put("/admin/{user_id}/reject")
async def admin_reject_kyc(
    user_id: uuid.UUID,
    request: Request,
    body: AdminRejectBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Admin: manually reject a user's KYC with a reason."""
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Admin access required")

    stmt = select(User).where(User.id == user_id)
    result = await db.execute(stmt)
    target = result.scalar_one_or_none()

    if not target:
        raise HTTPException(status_code=404, detail="User not found")

    previous_kyc_status = target.kyc_status
    target.kyc_status = "REJECTED"
    target.kyc_rejection_reason = body.reason
    await record_audit(
        db,
        user_id=current_user.id,
        action=KYC_REJECTED,
        resource_type="kyc",
        resource_id=target.id,
        changes={
            "kyc_record_id": str(target.id),
            "target_user_id": str(target.id),
            "kyc_status": {"from": previous_kyc_status, "to": "REJECTED"},
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(target)

    await send_kyc_rejected_email(target.email, target.first_name or "there", body.reason)

    return {
        "user_id": str(target.id),
        "kyc_status": target.kyc_status,
        "rejection_reason": target.kyc_rejection_reason,
        "message": "KYC rejected.",
    }
