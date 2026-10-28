"""Response schema for GET /api/auth/me/abilities (docs/user-abilities.md)."""
from pydantic import BaseModel

from app.services.action_policy import AbilityId, ReasonCode


class AbilityResponse(BaseModel):
    action: AbilityId
    allowed: bool
    reason_code: ReasonCode | None = None
    reason: str | None = None


class AbilitiesResponse(BaseModel):
    schema_version: int
    abilities: list[AbilityResponse]
