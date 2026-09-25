"""Authenticated browsing intake and admin per-user activity timeline."""

from __future__ import annotations

import hashlib
from enum import IntEnum
from typing import Annotated, Literal
from uuid import UUID

from fastapi import APIRouter, Depends, HTTPException, Query, Request, status
from slowapi.util import get_remote_address
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.middleware.rbac import require_role
from app.models.user import User, UserRole
from app.rate_limit import limiter
from app.routers.auth_simple import get_current_user
from app.routing import BodySizeLimitRoute
from app.schemas.errors import AUTH_RESPONSES
from app.schemas.user_activity import (
    BrowsingEventsAccepted,
    BrowsingEventsIn,
    UserActivityPage,
)
from app.services.user_activity import get_user_activity_page, ingest_browsing_events


MAX_ACTIVITY_BODY_BYTES = 16 * 1024


class _ActivityBodyLimitRoute(BodySizeLimitRoute):
    max_body_bytes = MAX_ACTIVITY_BODY_BYTES
    body_too_large_detail = "Activity payload must be 16 KB or smaller"


router = APIRouter(
    tags=["user-activity"],
    responses=AUTH_RESPONSES,
    route_class=_ActivityBodyLimitRoute,
)


class ActivityDays(IntEnum):
    SEVEN = 7
    THIRTY = 30
    NINETY = 90


async def _get_activity_user(
    request: Request,
    current_user: Annotated[User, Depends(get_current_user)],
) -> User:
    request.state.activity_rate_user_id = str(current_user.id)
    return current_user


def _activity_user_rate_key(request: Request) -> str:
    user_id = getattr(request.state, "activity_rate_user_id", None)
    return f"activity:user:{user_id}" if user_id else f"activity:ip:{get_remote_address(request)}"


def _admin_token_rate_key(request: Request) -> str:
    authorization = request.headers.get("Authorization", "")
    if authorization:
        return "tok:" + hashlib.sha256(authorization.encode("utf-8")).hexdigest()[:16]
    return get_remote_address(request)


@router.post(
    "/activity/events",
    response_model=BrowsingEventsAccepted,
    status_code=status.HTTP_202_ACCEPTED,
)
@limiter.limit("60/minute", key_func=_activity_user_rate_key)
async def post_activity_events(
    request: Request,
    payload: BrowsingEventsIn,
    current_user: Annotated[User, Depends(_get_activity_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> BrowsingEventsAccepted:
    accepted = await ingest_browsing_events(
        db,
        user_id=current_user.id,
        payload=payload,
    )
    return BrowsingEventsAccepted(accepted=accepted)


@router.get(
    "/admin/users/{user_id}/activity",
    response_model=UserActivityPage,
)
@limiter.limit("60/minute", key_func=_admin_token_rate_key)
async def get_admin_user_activity(
    request: Request,
    user_id: UUID,
    db: Annotated[AsyncSession, Depends(get_db)],
    current_user: Annotated[User, Depends(require_role(UserRole.ADMIN))],
    days: Annotated[ActivityDays, Query()] = ActivityDays.THIRTY,
    kind: Annotated[Literal["all", "browsing", "business", "login"], Query()] = "all",
    limit: Annotated[int, Query(ge=1, le=100)] = 50,
    offset: Annotated[int, Query(ge=0, le=10_000)] = 0,
) -> UserActivityPage:
    if await db.scalar(select(User.id).where(User.id == user_id)) is None:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND, detail="User not found")
    return await get_user_activity_page(
        db,
        user_id=user_id,
        days=int(days),
        kind=kind,
        limit=limit,
        offset=offset,
    )
