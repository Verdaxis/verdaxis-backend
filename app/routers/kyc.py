"""KYC document submission and administrative review."""

from typing import Annotated
import uuid
import re
from datetime import UTC, datetime

from fastapi import APIRouter, Depends, File, Form, HTTPException, Request, UploadFile, status
from pydantic import BaseModel, Field, field_validator
from slowapi.util import get_remote_address
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import Organization, User, UserRole, UserStatus
from app.rate_limit import limiter
from app.routing import BodySizeLimitRoute
from app.routers.auth_simple import get_current_user
from app.services.audit_actions import KYC_APPROVED, KYC_REJECTED, KYC_SUBMITTED
from app.services.audit_service import record_audit, request_audit_context
from app.services.email import send_kyc_approved_email, send_kyc_rejected_email
from app.services.kyc import KYCProviderUnavailable, KYC_REVIEW_REQUIRED, verify_document_with_gemini
from app.services.execution_invalidation import (
    invalidate_execution_state_for_request,
    publish_execution_invalidation,
)

# Honour the runtime-owned KYC size configuration (defaults: 10 MiB per document,
# 20 MiB per request) so the documented KYC_MAX_* settings stay authoritative.
MAX_KYC_DOCUMENT_BYTES = settings.KYC_MAX_FILE_BYTES
MAX_KYC_REQUEST_BODY_BYTES = settings.KYC_MAX_TOTAL_BYTES + 64 * 1024


class _KYCBodyLimitRoute(BodySizeLimitRoute):
    max_body_bytes = MAX_KYC_REQUEST_BODY_BYTES
    body_too_large_detail = "KYC submission must be 20 MB or smaller"


router = APIRouter(prefix="/kyc", tags=["KYC"], route_class=_KYCBodyLimitRoute)
_CONTENT_TYPE_ALIASES = {"image/jpg": "image/jpeg", "application/x-pdf": "application/pdf"}
_DOCUMENT_SIGNATURES = {
    "image/jpeg": lambda content: content.startswith(b"\xff\xd8\xff"),
    "image/png": lambda content: content.startswith(b"\x89PNG\r\n\x1a\n"),
    "application/pdf": lambda content: content.startswith(b"%PDF-"),
}
_REGISTRATION_NUMBER_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 ./_-]{1,99}$")


class AdminRejectBody(BaseModel):
    reason: str = Field(min_length=3, max_length=1000)

    @field_validator("reason")
    @classmethod
    def normalize_reason(cls, value: str) -> str:
        normalized = value.strip()
        if len(normalized) < 3:
            raise ValueError("reason must contain at least 3 non-whitespace characters")
        return normalized


class AdminApproveBody(BaseModel):
    external_evidence_reference: str = Field(min_length=3, max_length=200)
    review_note: str = Field(min_length=10, max_length=2000)

    @field_validator("external_evidence_reference")
    @classmethod
    def normalize_evidence_reference(cls, value: str) -> str:
        normalized = value.strip()
        if len(normalized) < 3 or any(ord(char) < 0x20 or ord(char) == 0x7F for char in normalized):
            raise ValueError("external_evidence_reference must be a printable external case reference")
        return normalized

    @field_validator("review_note")
    @classmethod
    def normalize_review_note(cls, value: str) -> str:
        normalized = value.strip()
        if len(normalized) < 10:
            raise ValueError("review_note must contain at least 10 non-whitespace characters")
        return normalized


def _normalized_identity_text(value: str) -> str:
    return " ".join(value.split()).casefold()


def _validate_declared_organization_fields(
    organization: Organization,
    *,
    declared_company_name: str | None,
    registration_number: str | None,
) -> None:
    if (
        declared_company_name is not None
        and _normalized_identity_text(declared_company_name)
        != _normalized_identity_text(organization.name)
    ):
        raise HTTPException(
            status_code=422,
            detail="Declared company name must match the current organization",
        )
    if (
        registration_number is not None
        and organization.tax_id
        and _normalized_identity_text(registration_number)
        != _normalized_identity_text(organization.tax_id)
    ):
        raise HTTPException(
            status_code=422,
            detail="Registration number must match the current organization record",
        )


def _require_current_kyc_organization(target: User) -> uuid.UUID:
    if (
        target.organization_id is None
        or target.kyc_organization_id is None
        or target.kyc_organization_id != target.organization_id
    ):
        raise HTTPException(
            status_code=409,
            detail={
                "code": "KYC_ORGANIZATION_MISMATCH",
                "message": "KYC evidence does not match the user's current organization",
            },
        )
    return target.organization_id


async def _read_validated_document(upload: UploadFile) -> tuple[bytes, str]:
    supplied_type = (upload.content_type or "").split(";", 1)[0].strip().lower()
    mime_type = _CONTENT_TYPE_ALIASES.get(supplied_type, supplied_type)
    signature_matches = _DOCUMENT_SIGNATURES.get(mime_type)
    if signature_matches is None:
        raise HTTPException(status_code=status.HTTP_415_UNSUPPORTED_MEDIA_TYPE, detail="KYC documents must be JPEG, PNG, or PDF files")
    content = await upload.read(MAX_KYC_DOCUMENT_BYTES + 1)
    if len(content) > MAX_KYC_DOCUMENT_BYTES:
        raise HTTPException(status_code=status.HTTP_413_CONTENT_TOO_LARGE, detail="Each KYC document must be 10 MB or smaller")
    if not content:
        raise HTTPException(status_code=400, detail="KYC documents cannot be empty")
    if not signature_matches(content):
        raise HTTPException(status_code=status.HTTP_415_UNSUPPORTED_MEDIA_TYPE, detail="KYC document content does not match its declared file type")
    return content, mime_type


async def _get_kyc_current_user(request: Request, current_user: Annotated[User, Depends(get_current_user)]) -> User:
    request.state.kyc_rate_user_id = str(current_user.id)
    return current_user


def _kyc_user_rate_key(request: Request) -> str:
    user_id = getattr(request.state, "kyc_rate_user_id", None)
    return f"kyc:user:{user_id}" if user_id else f"kyc:ip:{get_remote_address(request)}"


@router.post("/submit")
@limiter.limit("5/hour", key_func=_kyc_user_rate_key)
async def submit_kyc(
    request: Request,
    current_user: Annotated[User, Depends(_get_kyc_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
    passport: UploadFile = File(...),
    company_doc: UploadFile = File(...),
    declared_company_name: str | None = Form(None, max_length=200),
    registration_number: str | None = Form(None, max_length=100),
):
    if declared_company_name is not None:
        declared_company_name = declared_company_name.strip()
        if not declared_company_name:
            raise HTTPException(status_code=422, detail="Declared company name cannot be blank")
    if registration_number is not None:
        registration_number = registration_number.strip()
        if not _REGISTRATION_NUMBER_RE.fullmatch(registration_number):
            raise HTTPException(status_code=422, detail="Registration number contains invalid characters")

    passport_bytes, passport_mime = await _read_validated_document(passport)
    company_bytes, company_mime = await _read_validated_document(company_doc)
    user_id = current_user.id
    if current_user.organization_id is None:
        raise HTTPException(status_code=403, detail="Organization membership is required for KYC submission")
    organization_id = current_user.organization_id
    organization = (
        await db.execute(select(Organization).where(Organization.id == organization_id))
    ).scalar_one_or_none()
    if organization is None:
        raise HTTPException(status_code=403, detail="Organization membership is no longer available")
    _validate_declared_organization_fields(
        organization,
        declared_company_name=declared_company_name,
        registration_number=registration_number,
    )
    await db.rollback()
    advisory_results: dict[str, object] = {"provider": "gemini", "status": "unavailable"}
    try:
        passport_result = await verify_document_with_gemini(passport_bytes, passport_mime, "passport or government-issued ID")
        company_result = await verify_document_with_gemini(company_bytes, company_mime, "company registration document")
        advisory_results = {
            "provider": "gemini",
            "status": "advisory_complete",
            "passport_passed": bool(passport_result.get("passed")),
            "company_passed": bool(company_result.get("passed")),
        }
    except KYCProviderUnavailable:
        # Provider availability never changes the authoritative workflow.
        # Admin review remains required and can proceed from the submitted
        # evidence even when Gemini is unavailable.
        passport_result = company_result = None

    result = await db.execute(select(User).where(User.id == user_id).with_for_update())
    current_user = result.scalar_one_or_none()
    if current_user is None:
        raise HTTPException(status_code=401, detail="User account is no longer available")
    if current_user.organization_id != organization_id:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "KYC_MEMBERSHIP_CHANGED",
                "message": "Organization membership changed during submission; submit again",
            },
        )
    if current_user.status != UserStatus.APPROVED or current_user.must_change_password:
        raise HTTPException(status_code=403, detail="Account is not eligible for KYC submission")
    organization = (
        await db.execute(
            select(Organization).where(Organization.id == organization_id).with_for_update()
        )
    ).scalar_one_or_none()
    if organization is None:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "KYC_MEMBERSHIP_CHANGED",
                "message": "Organization membership changed during submission; submit again",
            },
        )
    _validate_declared_organization_fields(
        organization,
        declared_company_name=declared_company_name,
        registration_number=registration_number,
    )

    previous = current_user.kyc_status
    current_user.kyc_status = KYC_REVIEW_REQUIRED
    current_user.kyc_organization_id = organization_id
    current_user.kyc_rejection_reason = None
    current_user.kyc_external_evidence_reference = None
    current_user.kyc_review_note = None
    current_user.kyc_reviewed_by = None
    current_user.kyc_reviewed_at = None
    await record_audit(db, user_id=current_user.id, action=KYC_SUBMITTED, resource_type="kyc", resource_id=current_user.id, changes={
        "kyc_status": {"from": previous, "to": KYC_REVIEW_REQUIRED},
        "advisory": advisory_results,
        "declared_company_name_provided": bool(declared_company_name),
        "registration_number_provided": bool(registration_number),
        "evidence_limits": "Automated checks do not establish ownership or legal authenticity.",
    }, **request_audit_context(request))
    await db.commit()
    return {"kyc_status": current_user.kyc_status, "message": "KYC submitted for trusted administrator review."}


@router.get("/status")
async def get_kyc_status(current_user: Annotated[User, Depends(get_current_user)]):
    return {"email_verified": current_user.email_verified, "kyc_status": current_user.kyc_status, "kyc_organization_id": str(current_user.kyc_organization_id) if current_user.kyc_organization_id else None, "kyc_rejection_reason": current_user.kyc_rejection_reason}


@router.put("/admin/{user_id}/approve")
async def admin_approve_kyc(
    user_id: uuid.UUID,
    request: Request,
    body: AdminApproveBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Admin access required")
    target = (
        await db.execute(select(User).where(User.id == user_id).with_for_update())
    ).scalar_one_or_none()
    if not target:
        raise HTTPException(status_code=404, detail="User not found")
    if target.kyc_status != KYC_REVIEW_REQUIRED:
        raise HTTPException(
            status_code=409,
            detail={
                "code": "KYC_REVIEW_NOT_SUBMITTED",
                "message": "KYC must be submitted for review before approval",
            },
        )
    _require_current_kyc_organization(target)
    previous = target.kyc_status
    target.kyc_status = "APPROVED"
    target.kyc_rejection_reason = None
    target.kyc_external_evidence_reference = body.external_evidence_reference
    target.kyc_review_note = body.review_note
    target.kyc_reviewed_by = current_user.id
    target.kyc_reviewed_at = datetime.now(UTC)
    await record_audit(
        db,
        user_id=current_user.id,
        action=KYC_APPROVED,
        resource_type="kyc",
        resource_id=target.id,
        changes={
            "kyc_status": {"from": previous, "to": "APPROVED"},
            "external_evidence_reference": body.external_evidence_reference,
            "review_note": body.review_note,
            "raw_documents_retained_in_platform": False,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(target)
    await send_kyc_approved_email(target.email, target.first_name or "there")
    return {"user_id": str(target.id), "kyc_status": target.kyc_status, "account_status": target.status.value, "message": "KYC approved."}


@router.put("/admin/{user_id}/reject")
async def admin_reject_kyc(user_id: uuid.UUID, request: Request, body: AdminRejectBody, current_user: Annotated[User, Depends(get_current_user)], db: Annotated[AsyncSession, Depends(get_db)]):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Admin access required")
    target = (
        await db.execute(select(User).where(User.id == user_id).with_for_update())
    ).scalar_one_or_none()
    if not target:
        raise HTTPException(status_code=404, detail="User not found")
    _require_current_kyc_organization(target)
    previous = target.kyc_status
    target.kyc_status = "REJECTED"
    target.kyc_rejection_reason = body.reason
    target.kyc_review_note = body.reason
    target.kyc_reviewed_by = current_user.id
    target.kyc_reviewed_at = datetime.now(UTC)
    audit_context = request_audit_context(request)
    counts = await invalidate_execution_state_for_request(
        db,
        user_ids=[target.id],
        actor_user_id=current_user.id,
        reason="kyc_rejected",
        **audit_context,
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=KYC_REJECTED,
        resource_type="kyc",
        resource_id=target.id,
        changes={
            "kyc_status": {"from": previous, "to": "REJECTED"},
            "reason": body.reason,
            **counts,
        },
        **request_audit_context(request),
    )
    await db.commit()
    await publish_execution_invalidation(counts)
    await db.refresh(target)
    await send_kyc_rejected_email(target.email, target.first_name or "there", body.reason)
    return {"user_id": str(target.id), "kyc_status": target.kyc_status, "rejection_reason": target.kyc_rejection_reason, "message": "KYC rejected."}
