"""In-app feedback: identified, voluntary, first-party (docs/feedback.md).

Deliberately an operational support surface like admin user management — the
product-analytics suppression and no-identity rules do not apply here, and
nothing is forwarded to Umami.
"""
from datetime import UTC, datetime
import uuid

from fastapi import APIRouter, Depends, Query, Request
from pydantic import BaseModel, Field, field_validator
from sqlalchemy import func, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.models.feedback import FeedbackEntry
from app.models.user import Organization, User, UserRole
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user

router = APIRouter(tags=["Feedback"])

MAX_MESSAGE_CHARS = 2000
MAX_PAGE_CHARS = 200


class FeedbackSubmission(BaseModel):
    message: str = Field(min_length=1, max_length=MAX_MESSAGE_CHARS)
    page: str | None = Field(default=None, max_length=MAX_PAGE_CHARS)

    @field_validator("message")
    @classmethod
    def message_not_blank(cls, value: str) -> str:
        stripped = value.strip()
        if not stripped:
            raise ValueError("message must not be blank")
        return stripped

    @field_validator("page")
    @classmethod
    def page_is_bare_path(cls, value: str | None) -> str | None:
        # Path only — never a full URL, query string, or fragment (mirrors the
        # reliability telemetry rule). Invalid metadata is dropped, not a
        # reason to reject the message itself.
        if value is None:
            return None
        if not value.startswith("/") or "?" in value or "#" in value or "//" in value:
            return None
        return value


class FeedbackCreated(BaseModel):
    id: uuid.UUID
    created_at: datetime


class AdminFeedbackEntry(BaseModel):
    id: uuid.UUID
    created_at: datetime
    message: str
    page: str | None
    user_email: str | None
    user_name: str | None
    org_name: str | None


class AdminFeedbackResponse(BaseModel):
    items: list[AdminFeedbackEntry]
    total: int


@router.post("/feedback", response_model=FeedbackCreated, status_code=201)
@limiter.limit("5/minute")
async def submit_feedback(
    request: Request,
    submission: FeedbackSubmission,
    current_user: User = Depends(get_current_user),
    db: AsyncSession = Depends(get_db),
):
    entry = FeedbackEntry(
        id=uuid.uuid4(),
        user_id=current_user.id,
        message=submission.message,
        page=submission.page,
        created_at=datetime.now(UTC),
    )
    db.add(entry)
    await db.commit()
    return FeedbackCreated(id=entry.id, created_at=entry.created_at)


@router.get("/admin/feedback", response_model=AdminFeedbackResponse)
@limiter.limit("60/minute")
async def list_feedback(
    request: Request,
    db: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.ADMIN)),
    limit: int = Query(50, ge=1, le=200),
    offset: int = Query(0, ge=0),
):
    base = (
        select(FeedbackEntry, User.email, User.first_name, User.last_name, Organization.name)
        .outerjoin(User, FeedbackEntry.user_id == User.id)
        .outerjoin(Organization, User.organization_id == Organization.id)
    )
    total_q = await db.execute(select(func.count()).select_from(base.subquery()))
    total = total_q.scalar() or 0

    rows = await db.execute(
        base.order_by(FeedbackEntry.created_at.desc()).limit(limit).offset(offset)
    )
    items = [
        AdminFeedbackEntry(
            id=entry.id,
            created_at=entry.created_at,
            message=entry.message,
            page=entry.page,
            user_email=email,
            user_name=" ".join(part for part in (first_name, last_name) if part) or None,
            org_name=org_name,
        )
        for entry, email, first_name, last_name, org_name in rows
    ]
    return AdminFeedbackResponse(items=items, total=total)
