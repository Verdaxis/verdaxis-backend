"""
OAuth2 client self-service API — S7-002, S7-003, S7-004.

Endpoints:
  POST   /api/oauth/clients       — create a new API client (returns secret once)
  GET    /api/oauth/clients       — list current user's clients
  DELETE /api/oauth/clients/{id}  — revoke a client
  POST   /api/oauth/token         — client_credentials grant → JWT
"""
import secrets
import uuid
from datetime import timedelta
from typing import Annotated

import bcrypt
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.database import get_db
from app.models.oauth import OAuthClient
from app.models.user import User
from app.routers.auth_simple import get_current_user
from app.core.security import create_access_token
from app.schemas.oauth import (
    OAuthClientCreate,
    OAuthClientCreateResponse,
    OAuthClientResponse,
    OAuthTokenRequest,
    OAuthTokenResponse,
)

router = APIRouter(prefix="/oauth", tags=["OAuth2"])

# OAuth2 access tokens for API clients live longer than user session tokens
_OAUTH_TOKEN_EXPIRE_MINUTES = 60


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _hash_secret(secret: str) -> str:
    return bcrypt.hashpw(secret.encode("utf-8"), bcrypt.gensalt()).decode("utf-8")


def _verify_secret(plain: str, hashed: str) -> bool:
    return bcrypt.checkpw(plain.encode("utf-8"), hashed.encode("utf-8"))


# ---------------------------------------------------------------------------
# S7-002: Client CRUD
# ---------------------------------------------------------------------------

@router.post("/clients", response_model=OAuthClientCreateResponse, status_code=201)
async def create_client(
    body: OAuthClientCreate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Create a new OAuth2 API client. Returns the client_secret once — store it securely."""
    plain_secret = secrets.token_urlsafe(40)
    client = OAuthClient(
        name=body.name,
        scopes=body.scopes,
        client_secret_hash=_hash_secret(plain_secret),
        created_by=current_user.id,
    )
    db.add(client)
    await db.commit()
    await db.refresh(client)

    return OAuthClientCreateResponse(
        client_id=client.client_id,
        name=client.name,
        scopes=client.scopes,
        rate_limit_tier=client.rate_limit_tier,
        created_at=client.created_at,
        client_secret=plain_secret,
    )


@router.get("/clients", response_model=list[OAuthClientResponse])
async def list_clients(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """List API clients owned by the current user."""
    result = await db.execute(
        select(OAuthClient).where(OAuthClient.created_by == current_user.id)
    )
    return result.scalars().all()


@router.delete("/clients/{client_id}", status_code=204)
async def delete_client(
    client_id: uuid.UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Revoke an API client. Only the owner can delete their own clients."""
    result = await db.execute(
        select(OAuthClient).where(OAuthClient.client_id == client_id)
    )
    client = result.scalar_one_or_none()
    if client is None:
        raise HTTPException(status_code=404, detail="Client not found")
    if client.created_by != current_user.id:
        raise HTTPException(status_code=403, detail="Not authorized to delete this client")
    await db.delete(client)
    await db.commit()


# ---------------------------------------------------------------------------
# S7-003: Token endpoint — client_credentials grant
# ---------------------------------------------------------------------------

@router.post("/token", response_model=OAuthTokenResponse)
async def issue_token(
    body: OAuthTokenRequest,
    db: AsyncSession = Depends(get_db),
):
    """
    OAuth2 client_credentials grant.
    Validates client_id + client_secret and returns a scoped JWT.
    """
    result = await db.execute(
        select(OAuthClient).where(OAuthClient.client_id == body.client_id)
    )
    client = result.scalar_one_or_none()

    if client is None or not _verify_secret(body.client_secret, client.client_secret_hash):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Invalid client credentials",
            headers={"WWW-Authenticate": "Bearer"},
        )

    scopes = client.scopes or []
    token = create_access_token(
        subject=str(client.client_id),
        expires_delta=timedelta(minutes=_OAUTH_TOKEN_EXPIRE_MINUTES),
        additional_claims={
            "type": "access",  # overridden explicitly
            "token_kind": "oauth2_client",
            "scopes": scopes,
            "rate_limit_tier": client.rate_limit_tier,
        },
    )

    return OAuthTokenResponse(
        access_token=token,
        expires_in=_OAUTH_TOKEN_EXPIRE_MINUTES * 60,
        scope=" ".join(scopes),
    )


# ---------------------------------------------------------------------------
# S7-004: Scope enforcement dependency
# ---------------------------------------------------------------------------

def require_scope(scope: str):
    """
    FastAPI dependency factory that enforces a required scope on the current token.

    - OAuth2 client tokens (token_kind == "oauth2_client"): must contain `scope` in
      the `scopes` claim.
    - User login tokens (no token_kind): pass all scope checks for backward compat.
    """
    from fastapi.security import OAuth2PasswordBearer

    oauth2_scheme = OAuth2PasswordBearer(tokenUrl="/api/auth/login")

    async def _dependency(token: Annotated[str, Depends(oauth2_scheme)]):
        from app.core.security import decode_token
        try:
            payload = decode_token(token)
        except Exception:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Could not validate credentials",
                headers={"WWW-Authenticate": "Bearer"},
            )

        # User login tokens bypass scope checks (backward compat)
        token_kind = payload.get("token_kind")
        if token_kind != "oauth2_client":
            return payload

        # OAuth2 client tokens must have the required scope
        token_scopes: list[str] = payload.get("scopes", [])
        if scope not in token_scopes:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail=f"Token missing required scope: '{scope}'",
            )
        return payload

    return Depends(_dependency)
