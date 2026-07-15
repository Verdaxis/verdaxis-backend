"""Shared error response schema for OpenAPI declarations.

FastAPI's HTTPException serializes to {"detail": "..."} — this model gives
that shape a name so routers can declare 401/403/429 responses in the schema
instead of leaving consumers to guess.
"""
from pydantic import BaseModel, Field


class ErrorDetail(BaseModel):
    detail: str = Field(description="Human-readable explanation of the error.")


AUTH_RESPONSES = {
    401: {"model": ErrorDetail, "description": "Missing, expired, or invalid credentials."},
    403: {"model": ErrorDetail, "description": "Authenticated but not permitted (wrong role or account not approved)."},
    429: {"model": ErrorDetail, "description": "Rate limit exceeded."},
}
