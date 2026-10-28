"""What the current user may do, and why not (server-driven UI hints)."""
from typing import Annotated

from fastapi import APIRouter, Depends, Response
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.user import User
from app.routers.auth_simple import get_current_user
from app.schemas.abilities import AbilitiesResponse, AbilityResponse
from app.services.abilities import SCHEMA_VERSION, compute_abilities

router = APIRouter(prefix="/auth/me", tags=["auth"])


@router.get("/abilities", response_model=AbilitiesResponse)
async def read_my_abilities(
    response: Response,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> AbilitiesResponse:
    """Return every ability with its allowed flag and denial reason.

    Read-only. These are hints: each endpoint remains authoritative. A separate
    route keeps ``/auth/me`` small; this one loads the organization and, for
    administrators, staff capabilities.
    """
    response.headers["Cache-Control"] = "no-store"
    abilities = await compute_abilities(db, current_user)
    return AbilitiesResponse(
        schema_version=SCHEMA_VERSION,
        abilities=[
            AbilityResponse(
                action=ability.action,
                allowed=ability.allowed,
                reason_code=ability.reason_code,
                reason=ability.reason,
            )
            for ability in abilities
        ],
    )
