"""
OAuth 2.0 / OIDC login router for Google and Microsoft SSO.

Uses Authlib as the OIDC client. On successful callback, issues Verdaxis
access + refresh tokens via the existing PyJWT dual-token system.

Endpoints live at /oauth/* (no /api prefix) so Caddy can route them
directly to the backend without conflicting with the SPA catch-all.
"""
import uuid
import structlog
from datetime import datetime, UTC

from authlib.integrations.starlette_client import OAuth, OAuthError
from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import RedirectResponse
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.config import settings
from app.core.security import create_access_token, create_refresh_token
from app.database import get_db
from app.models.user import Organization, User, UserRole, UserStatus

logger = structlog.get_logger()

# ---------------------------------------------------------------------------
# Authlib OAuth client setup
# ---------------------------------------------------------------------------

oauth = OAuth()

# Google OIDC — uses auto-discovery via .well-known/openid-configuration
oauth.register(
    name="google",
    client_id=settings.GOOGLE_CLIENT_ID,
    client_secret=settings.GOOGLE_CLIENT_SECRET,
    server_metadata_url="https://accounts.google.com/.well-known/openid-configuration",
    client_kwargs={"scope": "openid email profile"},
)

# Microsoft Entra ID (Azure AD) — common tenant for multi-org login
oauth.register(
    name="microsoft",
    client_id=settings.MICROSOFT_CLIENT_ID,
    client_secret=settings.MICROSOFT_CLIENT_SECRET,
    server_metadata_url="https://login.microsoftonline.com/common/v2.0/.well-known/openid-configuration",
    client_kwargs={"scope": "openid email profile"},
)

# ---------------------------------------------------------------------------
# Router
# ---------------------------------------------------------------------------

router = APIRouter(prefix="/oauth", tags=["OAuth SSO"])

FRONTEND_APP_URL = "https://app.verdaxis.exchange/app"
FRONTEND_LOGIN_URL = "https://app.verdaxis.exchange/login"


async def _find_or_create_user(
    db: AsyncSession,
    email: str,
    first_name: str | None,
    last_name: str | None,
    provider: str,
) -> User:
    """
    Look up a user by email. If they exist, update oauth_provider and
    last_login. If not, auto-create with status=PENDING and link to
    an organization if the email domain matches.
    """
    stmt = select(User).where(User.email == email)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if user is not None:
        # Existing user — record the OAuth provider if not already set
        if user.oauth_provider is None:
            user.oauth_provider = provider
        user.last_login = datetime.now(UTC)
        await db.commit()
        await db.refresh(user)
        return user

    # New user — check if their email domain matches an organization
    email_domain = email.split("@")[1].lower()
    org_stmt = select(Organization).where(Organization.domain == email_domain)
    org_result = await db.execute(org_stmt)
    org = org_result.scalar_one_or_none()

    new_user = User(
        id=uuid.uuid4(),
        email=email,
        password_hash=None,  # OAuth users have no password
        first_name=first_name,
        last_name=last_name,
        role=UserRole.BUYER,  # Default role for self-service signups
        status=UserStatus.PENDING,
        organization_id=org.id if org else None,
        oauth_provider=provider,
        last_login=datetime.now(UTC),
    )

    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)
    logger.info(
        "oauth_user_created",
        email=email,
        provider=provider,
        org_linked=org is not None,
    )
    return new_user


def _build_redirect(user: User) -> RedirectResponse:
    """Build a redirect to the frontend with Verdaxis tokens in the query string."""
    access_token = create_access_token(
        subject=str(user.id),
        additional_claims={"role": user.role.value if user.role else None},
    )
    refresh_token = create_refresh_token(subject=str(user.id))

    redirect_url = (
        f"{FRONTEND_APP_URL}"
        f"?token={access_token}"
        f"&refresh={refresh_token}"
    )
    return RedirectResponse(url=redirect_url)


# ---------------------------------------------------------------------------
# Google
# ---------------------------------------------------------------------------

@router.get("/google/login")
async def google_login(request: Request):
    """Redirect to Google OAuth consent screen."""
    if not settings.GOOGLE_CLIENT_ID:
        raise HTTPException(status_code=501, detail="Google OAuth not configured")
    redirect_uri = f"{settings.OAUTH_REDIRECT_BASE}/oauth/google/callback"
    return await oauth.google.authorize_redirect(request, redirect_uri)


@router.get("/google/callback")
async def google_callback(request: Request, db: AsyncSession = Depends(get_db)):
    """Handle Google OAuth callback — exchange code, find/create user, issue tokens."""
    try:
        token = await oauth.google.authorize_access_token(request)
    except OAuthError as e:
        logger.warning("oauth_google_error", error=str(e))
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_failed")

    userinfo = token.get("userinfo")
    if userinfo is None:
        logger.warning("oauth_google_no_userinfo")
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_no_profile")

    email = userinfo.get("email")
    if not email:
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_no_email")

    user = await _find_or_create_user(
        db=db,
        email=email,
        first_name=userinfo.get("given_name"),
        last_name=userinfo.get("family_name"),
        provider="google",
    )

    # If user is not approved yet, redirect to login with a status message
    if user.status != UserStatus.APPROVED:
        return RedirectResponse(
            url=f"{FRONTEND_LOGIN_URL}?error=account_pending"
        )

    return _build_redirect(user)


# ---------------------------------------------------------------------------
# Microsoft
# ---------------------------------------------------------------------------

@router.get("/microsoft/login")
async def microsoft_login(request: Request):
    """Redirect to Microsoft Entra ID consent screen."""
    if not settings.MICROSOFT_CLIENT_ID:
        raise HTTPException(status_code=501, detail="Microsoft OAuth not configured")
    redirect_uri = f"{settings.OAUTH_REDIRECT_BASE}/oauth/microsoft/callback"
    return await oauth.microsoft.authorize_redirect(request, redirect_uri)


@router.get("/microsoft/callback")
async def microsoft_callback(request: Request, db: AsyncSession = Depends(get_db)):
    """Handle Microsoft OAuth callback — exchange code, find/create user, issue tokens."""
    try:
        token = await oauth.microsoft.authorize_access_token(request)
    except OAuthError as e:
        logger.warning("oauth_microsoft_error", error=str(e))
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_failed")

    userinfo = token.get("userinfo")
    if userinfo is None:
        logger.warning("oauth_microsoft_no_userinfo")
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_no_profile")

    email = userinfo.get("email")
    if not email:
        return RedirectResponse(url=f"{FRONTEND_LOGIN_URL}?error=oauth_no_email")

    user = await _find_or_create_user(
        db=db,
        email=email,
        first_name=userinfo.get("given_name"),
        last_name=userinfo.get("family_name"),
        provider="microsoft",
    )

    if user.status != UserStatus.APPROVED:
        return RedirectResponse(
            url=f"{FRONTEND_LOGIN_URL}?error=account_pending"
        )

    return _build_redirect(user)
