"""
Pydantic schemas for the OAuth2 client self-service API — S7-001/S7-002/S7-003.
"""
import uuid
from datetime import datetime

from pydantic import BaseModel, field_validator


# ---------------------------------------------------------------------------
# Valid scopes
# ---------------------------------------------------------------------------

VALID_SCOPES = {"read:market", "read:orders", "write:orders", "read:compliance"}

DEFAULT_SCOPES = ["read:market"]


# ---------------------------------------------------------------------------
# Client management
# ---------------------------------------------------------------------------

class OAuthClientCreate(BaseModel):
    name: str
    scopes: list[str] = DEFAULT_SCOPES

    @field_validator("scopes")
    @classmethod
    def validate_scopes(cls, v: list[str]) -> list[str]:
        invalid = set(v) - VALID_SCOPES
        if invalid:
            raise ValueError(f"Invalid scopes: {invalid}. Valid: {VALID_SCOPES}")
        return v


class OAuthClientResponse(BaseModel):
    client_id: uuid.UUID
    name: str
    scopes: list[str]
    rate_limit_tier: str
    created_at: datetime

    class Config:
        from_attributes = True


class OAuthClientCreateResponse(OAuthClientResponse):
    """Returned ONCE on client creation — includes the unhashed secret."""
    client_secret: str


# ---------------------------------------------------------------------------
# Token endpoint
# ---------------------------------------------------------------------------

class OAuthTokenRequest(BaseModel):
    grant_type: str
    client_id: uuid.UUID
    client_secret: str

    @field_validator("grant_type")
    @classmethod
    def validate_grant_type(cls, v: str) -> str:
        if v != "client_credentials":
            raise ValueError("Only 'client_credentials' grant type is supported")
        return v


class OAuthTokenResponse(BaseModel):
    access_token: str
    token_type: str = "bearer"
    expires_in: int
    scope: str
