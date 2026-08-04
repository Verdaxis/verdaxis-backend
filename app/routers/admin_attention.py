"""Admin view of the onboarding attention classification.

Thin read-only wrapper over app.services.onboarding_attention — the exact
stage rules the external alert timer uses. This is an operational outreach
surface (identified, like admin user management), not product analytics.
"""
from datetime import UTC, datetime

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.models.user import User, UserRole
from app.rate_limit import limiter
from app.services.onboarding_attention import classify_candidate, load_candidates

router = APIRouter(tags=["Admin"])


class AttentionItem(BaseModel):
    email: str
    name: str | None
    role: str | None
    stage: str
    since: datetime
    organization_name: str | None
    last_login: datetime | None


class AttentionResponse(BaseModel):
    items: list[AttentionItem]
    generated_at: datetime


@router.get("/admin/onboarding-attention", response_model=AttentionResponse)
@limiter.limit("60/minute")
async def list_onboarding_attention(
    request: Request,
    db: AsyncSession = Depends(get_db),
    current_user: User = Depends(require_role(UserRole.ADMIN)),
):
    now = datetime.now(UTC)
    items: list[AttentionItem] = []
    for candidate in await load_candidates(db):
        attention = classify_candidate(candidate, now)
        if attention is None:
            continue
        name = " ".join(
            part for part in (candidate.first_name, candidate.last_name) if part
        ) or None
        items.append(
            AttentionItem(
                email=candidate.email,
                name=name,
                role=candidate.role,
                stage=attention.stage.value,
                since=attention.since,
                organization_name=candidate.organization_name,
                last_login=candidate.last_login,
            )
        )
    items.sort(key=lambda item: item.since)
    return AttentionResponse(items=items, generated_at=now)
