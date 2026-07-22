from fastapi import Request as _Request
from app.rate_limit import limiter
from datetime import datetime, timedelta, UTC
import logging
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, Query, Response, status
from fastapi.responses import JSONResponse
from fastapi.security import OAuth2PasswordBearer, OAuth2PasswordRequestForm
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import func, or_, select, text, update
from sqlalchemy.exc import DBAPIError, IntegrityError, SQLAlchemyError
import hashlib
import jwt
import re
import secrets
from dataclasses import dataclass
from app.database import get_db
from app.config import settings
from app.models.user import User, UserRole, UserStatus, Organization
from app.models.refresh_session import RefreshSession
from app.models.registration import PendingRegistration, OrganizationJoinRequest, JoinRequestStatus
from app.schemas.user import UserCreate, UserResponse, UserUpdate, RegistrationResponse, PasswordChangeRequest
from app.schemas.organization import OrganizationCreate
from app.schemas.errors import AUTH_RESPONSES
from app.core.security import (
    verify_password, get_password_hash,
    create_access_token, create_refresh_token, create_stream_token, decode_token,
    REFRESH_TOKEN_EXPIRE_DAYS,
    hash_token_identifier,
    validate_password_bytes as _validate_password_bytes,
    MAX_PASSWORD_BYTES,
)
from pydantic import BaseModel, ConfigDict, Field, field_validator
import uuid

from app.models.referral import Referral, ReferralStatus, generate_referral_code
from app.services.email_domains import registration_organization_domain
from app.services.email import send_verification_email, send_password_reset_email
from app.services.monitor_canary import is_monitor_canary_email
from app.services.audit_service import record_audit, request_audit_context
from app.services.product_analytics import is_retryable_transaction_error, record_login_day
from app.services.user_status_transition import record_initial_status, record_status_transition
from app.services.audit_actions import (
    ADMIN_USER_APPROVED,
    ADMIN_USER_REJECTED,
    ADMIN_ORGANIZATION_APPROVED,
    ADMIN_ORGANIZATION_REJECTED,
    USER_PASSWORD_CHANGED,
    USER_PASSWORD_RESET_COMPLETED,
    USER_PASSWORD_RESET_REQUESTED,
    USER_REGISTERED,
    ORGANIZATION_JOIN_REQUESTED,
    ORGANIZATION_JOIN_APPROVED,
    ORGANIZATION_JOIN_REJECTED,
    KYC_MEMBERSHIP_INVALIDATED,
)
from app.services.behavioral_analytics import (
    organization_created_event,
    registration_completed_event,
    track_analytics_event,
)
from app.routing import BodySizeLimitRoute, require_trusted_browser_origin
from app.services.market_events import enqueue_market_events
from app.services.market_invalidation import invalidate_organization_market_access
from app.services.market_transactions import retry_market_transaction

logger = logging.getLogger(__name__)

ADMIN_REVIEW_PROJECTION_FIELDS = frozenset(
    {
        "user_id", "email", "email_verified", "account_status", "role",
        "kyc_status", "kyc_organization_id", "kyc_external_evidence_reference",
        "kyc_review_note", "kyc_reviewed_by", "kyc_reviewed_at",
        "current_organization", "requested_organizations", "created_at",
    }
)
ADMIN_REVIEW_MAX_JOIN_ROWS_PER_USER = 20

class RegisterWithOrgRequest(BaseModel):
    registration_token: str
    organization: OrganizationCreate

class ResendVerificationRequest(BaseModel):
    email: str

    @field_validator("email")
    @classmethod
    def normalize_email(cls, value: str) -> str:
        return value.strip().lower()

class RefreshRequest(BaseModel):
    refresh_token: str | None = None

class ForgotPasswordRequest(BaseModel):
    email: str

    @field_validator("email")
    @classmethod
    def normalize_email(cls, value: str) -> str:
        return value.strip().lower()


class AdminDecisionBody(BaseModel):
    reason: str | None = None


class JoinReviewBody(BaseModel):
    review_note: str = Field(min_length=3, max_length=1000)

    @field_validator("review_note")
    @classmethod
    def normalize_review_note(cls, value: str) -> str:
        normalized = value.strip()
        if len(normalized) < 3:
            raise ValueError("review_note must contain at least 3 non-whitespace characters")
        return normalized

class ResetPasswordRequest(BaseModel):
    token: str
    new_password: str

    @field_validator("new_password")
    @classmethod
    def validate_password_size(cls, value: str) -> str:
        return validate_password_bytes(value)


class _AuthBodyLimitRoute(BodySizeLimitRoute):
    max_body_bytes = 128 * 1024
    body_too_large_detail = "Authentication request is too large"

router = APIRouter(
    prefix="/auth",
    tags=["Authentication"],
    responses=AUTH_RESPONSES,
    route_class=_AuthBodyLimitRoute,
)

REFRESH_COOKIE_NAME = "refresh_token"
REFRESH_COOKIE_PATH = "/api/auth"
REFRESH_COOKIE_MAX_AGE = REFRESH_TOKEN_EXPIRE_DAYS * 24 * 60 * 60
REFRESH_COOKIE_SAMESITE = "lax"
DEVICE_SESSION_COOKIE_NAME = "device_session"
DEVICE_SESSION_ID_PATTERN = re.compile(r"^[A-Za-z0-9_-]{43}$")
EMAIL_VERIFICATION_TTL = timedelta(hours=24)
REGISTRATION_TTL = timedelta(minutes=30)
REFRESH_ROTATION_GRACE = timedelta(seconds=5)
TERMINAL_REFRESH_ERROR_CODES = frozenset(
    {
        "REFRESH_TOKEN_MISSING",
        "REFRESH_TOKEN_EXPIRED",
        "REFRESH_TOKEN_INVALID",
        "REFRESH_SESSION_REVOKED",
        "REFRESH_TOKEN_REPLAYED",
        "REFRESH_ACCOUNT_INACTIVE",
        "REFRESH_PASSWORD_CHANGED",
        "REFRESH_DEVICE_REQUIRED",
    }
)
REFRESH_TRANSIENT_ERROR_CODES = frozenset({"REFRESH_ROTATION_IN_PROGRESS"})


def _terminal_refresh_error(code: str, message: str) -> HTTPException:
    if code not in TERMINAL_REFRESH_ERROR_CODES:
        raise ValueError("Unknown terminal refresh error code")
    return HTTPException(status_code=401, detail={"code": code, "message": message})


@dataclass(frozen=True)
class OpaqueRegistrationToken:
    raw: str
    token_hash: str
    expires_at: datetime


def _new_registration_token() -> OpaqueRegistrationToken:
    raw = secrets.token_urlsafe(32)
    return OpaqueRegistrationToken(
        raw=raw,
        token_hash=hash_token_identifier(raw),
        expires_at=datetime.now(UTC) + REGISTRATION_TTL,
    )


def validate_password_bytes(password: str) -> str:
    return _validate_password_bytes(password)

oauth2_scheme = OAuth2PasswordBearer(tokenUrl="auth/login")


def _build_token_pair(subject: str, *, family_id: str | None = None) -> tuple[str, str]:
    # Deliberately no role claim: authorization always reads the role from
    # the DB (require_role), so a claim here would only invite a future
    # regression where something trusts the client-visible token instead
    # (Sprint 3 item 3).
    access_token = create_access_token(subject=subject)
    refresh_token = create_refresh_token(subject=subject, family_id=family_id)
    return access_token, refresh_token


def _new_device_session_id() -> str:
    """Return a 256-bit opaque browser identifier; it is never logged or persisted raw."""
    return secrets.token_urlsafe(32)


def _valid_device_session_id(raw_device_id: str | None) -> bool:
    return bool(raw_device_id and DEVICE_SESSION_ID_PATTERN.fullmatch(raw_device_id))


def _device_session_hash(raw_device_id: str) -> str:
    if not _valid_device_session_id(raw_device_id):
        raise ValueError("Invalid browser device session identifier")
    return hash_token_identifier(raw_device_id)


def _device_advisory_lock_key(device_id_hash: str) -> int:
    """Map a SHA-256 device digest to PostgreSQL's signed 64-bit lock key."""
    unsigned = int(device_id_hash[:16], 16)
    return unsigned - (1 << 64) if unsigned >= (1 << 63) else unsigned


async def _acquire_device_session_lock(db: AsyncSession, device_id_hash: str) -> None:
    """Serialize every login/refresh/logout transaction for one browser device."""
    dialect_name = getattr(getattr(getattr(db, "bind", None), "dialect", None), "name", None)
    if dialect_name == "postgresql":
        await db.execute(
            text("SELECT pg_advisory_xact_lock(:device_lock_key)"),
            {"device_lock_key": _device_advisory_lock_key(device_id_hash)},
        )
        return
    if dialect_name == "sqlite" or settings.ENVIRONMENT.strip().lower() == "test":
        # SQLite is used only by isolated unit tests; deployed environments are
        # PostgreSQL and always take the transaction-scoped advisory lock.
        return
    raise RuntimeError("Browser device locking requires PostgreSQL")


def _refresh_payload_if_valid(raw_token: str | None) -> dict | None:
    if not raw_token:
        return None
    try:
        payload = decode_token(raw_token)
    except jwt.PyJWTError:
        return None
    return payload if payload.get("type") == "refresh" else None


async def _store_refresh_session(
    db: AsyncSession,
    user_id: uuid.UUID,
    refresh_token: str,
    *,
    device_id_hash: str,
) -> None:
    payload = decode_token(refresh_token)
    jti = payload.get("jti")
    family_id = payload.get("family_id")
    if not jti or not family_id:
        raise ValueError("Refresh token is missing durable rotation claims")
    try:
        family_uuid = uuid.UUID(str(family_id))
    except ValueError as exc:
        raise ValueError("Refresh token has an invalid family identifier") from exc
    db.add(
        RefreshSession(
            user_id=user_id,
            family_id=family_uuid,
            jti_hash=hash_token_identifier(jti),
            device_id_hash=device_id_hash,
            expires_at=datetime.fromtimestamp(payload["exp"], tz=UTC),
        )
    )


def _is_uuid(value: str) -> bool:
    try:
        uuid.UUID(value)
        return True
    except (ValueError, AttributeError, TypeError):
        return False


async def _rotate_refresh_session(
    db: AsyncSession,
    user_id: uuid.UUID,
    payload: dict,
    next_token: str,
    *,
    device_id_hash: str,
) -> None:
    jti = payload.get("jti")
    family_id = payload.get("family_id")
    if not jti or not family_id:
        raise _terminal_refresh_error(
            "REFRESH_TOKEN_INVALID",
            "Refresh token is missing durable session claims; sign in again",
        )
    try:
        family_uuid = uuid.UUID(str(family_id))
    except ValueError as exc:
        raise _terminal_refresh_error(
            "REFRESH_TOKEN_INVALID",
            "Refresh token has an invalid session family; sign in again",
        ) from exc
    # Device lock is always first. It orders database issuance independently
    # of browser response arrival, including cross-account multi-tab races.
    await _acquire_device_session_lock(db, device_id_hash)
    # The user row remains the serialization point shared with password and
    # admission changes after the device-level ordering boundary.
    locked_user = (
        await db.execute(select(User).where(User.id == user_id).with_for_update())
    ).scalar_one_or_none()
    if locked_user is None:
        raise _terminal_refresh_error(
            "REFRESH_ACCOUNT_INACTIVE",
            "Account is not active; sign in again after account access is restored",
        )
    result = await db.execute(
        select(RefreshSession).where(RefreshSession.jti_hash == hash_token_identifier(jti)).with_for_update()
    )
    session = result.scalar_one_or_none()
    if session is None or session.user_id != user_id or session.family_id != family_uuid:
        raise _terminal_refresh_error(
            "REFRESH_SESSION_REVOKED",
            "Refresh session is not active; sign in again",
        )
    if session.device_id_hash != device_id_hash:
        raise _terminal_refresh_error(
            "REFRESH_SESSION_REVOKED",
            "Refresh session is not active for this browser; sign in again",
        )
    now = datetime.now(UTC)
    if session.replaced_by_jti_hash:
        active_family_member = (
            await db.execute(
                select(RefreshSession)
                .where(
                    RefreshSession.family_id == session.family_id,
                    RefreshSession.revoked.is_(False),
                    RefreshSession.expires_at > now,
                    RefreshSession.device_id_hash == device_id_hash,
                )
                .with_for_update()
            )
        ).scalars().first()
        if active_family_member is None:
            # Logout/password reset/revocation may race a duplicate request
            # during the grace window. A 409 is valid only while a usable
            # successor still exists for the client to retry with.
            raise _terminal_refresh_error(
                "REFRESH_SESSION_REVOKED",
                "Refresh session has been revoked; sign in again",
            )
        if session.rotation_grace_until and session.rotation_grace_until >= now:
            # A second request from the same short-lived browser race is
            # rejected transiently without touching the valid successor or
            # its family. The client can retry using the successor it has.
            raise HTTPException(
                status_code=409,
                detail={
                    "code": "REFRESH_ROTATION_IN_PROGRESS",
                    "message": "Refresh token was already rotated; retry with the successor token",
                    "retry_after_seconds": 1,
                },
            )
        await db.execute(
            update(RefreshSession).where(RefreshSession.family_id == session.family_id).values(revoked=True)
        )
        await db.commit()
        raise _terminal_refresh_error(
            "REFRESH_TOKEN_REPLAYED",
            "Refresh token reuse was detected; sign in again",
        )
    if session.revoked:
        raise _terminal_refresh_error(
            "REFRESH_SESSION_REVOKED",
            "Refresh session has been revoked; sign in again",
        )
    next_payload = decode_token(next_token)
    session.revoked = True
    session.device_id_hash = device_id_hash
    session.replaced_by_jti_hash = hash_token_identifier(next_payload["jti"])
    session.rotation_grace_until = now + REFRESH_ROTATION_GRACE
    session.last_used_at = now
    await _store_refresh_session(
        db,
        user_id,
        next_token,
        device_id_hash=device_id_hash,
    )


def _refresh_cookie_secure() -> bool:
    # Only an explicitly selected local development environment may use an
    # insecure cookie. Unknown environment names stay secure.
    return settings.ENVIRONMENT.strip().lower() != "development"


def _set_refresh_cookie(response: Response, refresh_token: str) -> None:
    response.set_cookie(
        key=REFRESH_COOKIE_NAME,
        value=refresh_token,
        httponly=True,
        secure=_refresh_cookie_secure(),
        samesite=REFRESH_COOKIE_SAMESITE,
        max_age=REFRESH_COOKIE_MAX_AGE,
        path=REFRESH_COOKIE_PATH,
    )


def _set_device_session_cookie(response: Response, raw_device_id: str) -> None:
    response.set_cookie(
        key=DEVICE_SESSION_COOKIE_NAME,
        value=raw_device_id,
        httponly=True,
        secure=_refresh_cookie_secure(),
        samesite=REFRESH_COOKIE_SAMESITE,
        max_age=REFRESH_COOKIE_MAX_AGE,
        path=REFRESH_COOKIE_PATH,
    )


def _clear_refresh_cookie(response: Response) -> None:
    response.delete_cookie(
        key=REFRESH_COOKIE_NAME,
        path=REFRESH_COOKIE_PATH,
        httponly=True,
        secure=_refresh_cookie_secure(),
        samesite=REFRESH_COOKIE_SAMESITE,
    )


def _clear_device_session_cookie(response: Response) -> None:
    response.delete_cookie(
        key=DEVICE_SESSION_COOKIE_NAME,
        path=REFRESH_COOKIE_PATH,
        httponly=True,
        secure=_refresh_cookie_secure(),
        samesite=REFRESH_COOKIE_SAMESITE,
    )


def _clear_auth_cookies(response: Response) -> None:
    _clear_refresh_cookie(response)
    _clear_device_session_cookie(response)


def _new_email_verification_token() -> tuple[str, str, datetime]:
    raw_token = secrets.token_urlsafe(32)
    return raw_token, hash_token_identifier(raw_token), datetime.now(UTC) + EMAIL_VERIFICATION_TTL

PASSWORD_CHANGE_ALLOWED_PATHS = {
    "/api/auth/me",
    "/api/auth/me/password",
}


def _token_issued_at(payload: dict) -> datetime | None:
    issued_at_us = payload.get("iat_us")
    if isinstance(issued_at_us, int) and issued_at_us >= 0:
        return datetime.fromtimestamp(issued_at_us / 1_000_000, tz=UTC)
    issued_at = payload.get("iat")
    if isinstance(issued_at, (int, float)) and not isinstance(issued_at, bool):
        return datetime.fromtimestamp(issued_at, tz=UTC)
    return None


def _validate_password_cutoff(user: User, token_payload: dict) -> None:
    if user.password_changed_at is not None:
        issued_at = _token_issued_at(token_payload)
        cutoff = user.password_changed_at
        if cutoff.tzinfo is None:
            cutoff = cutoff.replace(tzinfo=UTC)
        if issued_at is None or issued_at <= cutoff:
            raise HTTPException(
                status_code=401,
                detail={
                    "code": "AUTH_PASSWORD_CHANGED",
                    "message": "Password was changed; sign in again",
                },
                headers={"WWW-Authenticate": "Bearer"},
            )


def validate_authenticated_user_state(user: User, token_payload: dict, *, request_path: str) -> None:
    """Validate mutable account state for an already-decoded signed token."""
    _validate_password_cutoff(user, token_payload)
    if user.status != UserStatus.APPROVED:
        raise HTTPException(status_code=403, detail=f"Account is {user.status.value}. Please wait for admin approval.")
    if user.must_change_password and request_path not in PASSWORD_CHANGE_ALLOWED_PATHS:
        raise HTTPException(status_code=403, detail="Password change required.")


async def _resolve_authenticated_user(
    request: _Request,
    token: str,
    db: AsyncSession,
) -> tuple[User, dict]:
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    try:
        payload = decode_token(token)
        # Only access tokens may authenticate ordinary API requests.
        if payload.get("type") != "access":
            raise credentials_exception
        user_id_str: str = payload.get("sub")
        if user_id_str is None:
            raise credentials_exception
    except jwt.PyJWTError:
        raise credentials_exception

    try:
        user_uuid = uuid.UUID(user_id_str)
    except ValueError:
        raise credentials_exception

    # This lock is also acquired by rotation/logout/password changes. Mutable
    # account checks and rotation therefore operate on one serialized state.
    stmt = select(User).where(User.id == user_uuid).with_for_update()
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if user is None:
        raise credentials_exception

    return user, payload


async def get_authenticated_user(
    request: _Request,
    token: Annotated[str, Depends(oauth2_scheme)],
    db: AsyncSession = Depends(get_db),
) -> User:
    """Authenticate token/account integrity without requiring execution admission.

    This dependency is intentionally limited to owner cleanup operations. A
    rejected user may withdraw their own outstanding state with an unexpired
    access token, but cannot create, accept, or progress executable state.
    """
    user, payload = await _resolve_authenticated_user(request, token, db)
    _validate_password_cutoff(user, payload)
    return user


async def get_current_user(
    request: _Request,
    token: Annotated[str, Depends(oauth2_scheme)],
    db: AsyncSession = Depends(get_db),
):
    user, payload = await _resolve_authenticated_user(request, token, db)
    validate_authenticated_user_state(user, payload, request_path=request.url.path)
    return user

# ---------------------------------------------------------------------------
# Login — returns access token in body and refresh token in both body + cookie
# ---------------------------------------------------------------------------

@router.post("/login")
@limiter.limit("5/minute")
async def login(
    request: _Request,
    response: Response,
    form_data: Annotated[OAuth2PasswordRequestForm, Depends()],
    db: AsyncSession = Depends(get_db),
):
    require_trusted_browser_origin(
        request,
        environment=settings.ENVIRONMENT,
        cookie_authenticated=bool(request.cookies.get(REFRESH_COOKIE_NAME)),
    )
    presented_refresh_payload = _refresh_payload_if_valid(
        request.cookies.get(REFRESH_COOKIE_NAME)
    )
    presented_device_id = request.cookies.get(DEVICE_SESSION_COOKIE_NAME)
    raw_device_id = (
        presented_device_id
        if _valid_device_session_id(presented_device_id)
        else _new_device_session_id()
    )
    device_id_hash = _device_session_hash(raw_device_id)
    normalized_email = str(form_data.username).strip().lower()
    login_instant = datetime.now(UTC)
    access_token = refresh_token = None
    for attempt in range(2):
        try:
            # Device lock precedes every account/session row. A login for B
            # therefore revokes any A family committed earlier for this
            # browser before B is issued, regardless Set-Cookie arrival order.
            await _acquire_device_session_lock(db, device_id_hash)
            user = (
                await db.execute(
                    select(User).where(User.email == normalized_email).with_for_update()
                )
            ).scalar_one_or_none()
            if not user or not verify_password(form_data.password, user.password_hash):
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED,
                    detail="Incorrect username or password",
                    headers={"WWW-Authenticate": "Bearer"},
                )
            if not user.email_verified:
                raise HTTPException(
                    status_code=status.HTTP_403_FORBIDDEN,
                    detail="Please verify your email address. Check your inbox for the verification link.",
                )
            if user.status != UserStatus.APPROVED:
                raise HTTPException(
                    status_code=status.HTTP_403_FORBIDDEN,
                    detail=f"Account is {user.status.value}. Please wait for admin approval.",
                )

            user.last_login = login_instant
            await record_login_day(db, user, at=login_instant)
            await _revoke_device_refresh_sessions(
                db,
                device_id_hash,
                presented_payload=presented_refresh_payload,
                lock_already_held=True,
            )
            access_token, refresh_token = _build_token_pair(str(user.id))
            await _store_refresh_session(
                db,
                user.id,
                refresh_token,
                device_id_hash=device_id_hash,
            )
            await db.commit()
            break
        except HTTPException:
            raise
        except DBAPIError as exc:
            await db.rollback()
            if attempt == 0 and is_retryable_transaction_error(exc):
                continue
            logger.error("login_session_persistence_failed (%s)", type(exc).__name__)
            raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable") from exc
        except (SQLAlchemyError, ValueError) as exc:
            await db.rollback()
            logger.error("login_session_persistence_failed (%s)", type(exc).__name__)
            raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable") from exc

    if access_token is None or refresh_token is None:
        # Defensive guard: the loop either committed a tracked pair or raised.
        raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable")
    _set_refresh_cookie(response, refresh_token)
    _set_device_session_cookie(response, raw_device_id)
    # The refresh token travels ONLY in the HttpOnly cookie — never in the
    # JSON body, where an XSS could read it.
    return {
        "access_token": access_token,
        "token_type": "bearer",
    }

# ---------------------------------------------------------------------------
# Refresh — exchange refresh token from body or cookie for new access + refresh tokens
# ---------------------------------------------------------------------------

@router.post("/refresh")
@limiter.limit("10/minute")
async def refresh_tokens(
    request: _Request,
    response: Response,
    body: RefreshRequest | None = None,
    db: AsyncSession = Depends(get_db),
):
    require_trusted_browser_origin(
        request,
        environment=settings.ENVIRONMENT,
        cookie_authenticated=bool(request.cookies.get(REFRESH_COOKIE_NAME)),
    )
    refresh_token = None
    if body and body.refresh_token:
        # TODO(remove-json-refresh) 2026-07-09: accepted only for older
        # cached bundles; the current frontend relies on the cookie.
        logger.warning("Deprecated JSON-body refresh token used; clients should rely on the HttpOnly cookie")
        refresh_token = body.refresh_token
    else:
        refresh_token = request.cookies.get(REFRESH_COOKIE_NAME)

    if not refresh_token:
        raise _terminal_refresh_error("REFRESH_TOKEN_MISSING", "Refresh token is required")

    raw_device_id = request.cookies.get(DEVICE_SESSION_COOKIE_NAME)
    if not _valid_device_session_id(raw_device_id):
        raise _terminal_refresh_error(
            "REFRESH_DEVICE_REQUIRED",
            "Browser session is missing or invalid; sign in again",
        )
    device_id_hash = _device_session_hash(raw_device_id)

    try:
        payload = decode_token(refresh_token)
        if payload.get("type") != "refresh":
            raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")
        user_id_str = payload.get("sub")
        if not user_id_str:
            raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")
    except jwt.ExpiredSignatureError:
        raise _terminal_refresh_error("REFRESH_TOKEN_EXPIRED", "Refresh token expired; sign in again")
    except jwt.PyJWTError:
        raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")

    try:
        user_uuid = uuid.UUID(user_id_str)
    except (ValueError, TypeError, AttributeError):
        raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")

    # The same device lock is acquired by login and logout before any user or
    # refresh-family row, giving all three paths one canonical lock order.
    await _acquire_device_session_lock(db, device_id_hash)
    stmt = select(User).where(User.id == user_uuid).with_for_update()
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user or user.status != UserStatus.APPROVED:
        raise _terminal_refresh_error(
            "REFRESH_ACCOUNT_INACTIVE",
            "Account is not active; sign in again after account access is restored",
        )

    # Check password_changed_at invalidation on refresh too
    if user.password_changed_at is not None:
        issued_at = _token_issued_at(payload)
        if issued_at is None:
            raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")
        cutoff = user.password_changed_at
        if cutoff.tzinfo is None:
            cutoff = cutoff.replace(tzinfo=UTC)
        if issued_at <= cutoff:
            raise _terminal_refresh_error(
                "REFRESH_PASSWORD_CHANGED",
                "Password changed after this session was issued; sign in again",
            )

    try:
        access_token, refresh_token = _build_token_pair(str(user.id), family_id=payload.get("family_id"))
        await _rotate_refresh_session(
            db,
            user.id,
            payload,
            refresh_token,
            device_id_hash=device_id_hash,
        )
        await db.commit()
    except HTTPException:
        raise
    except (SQLAlchemyError, ValueError) as exc:
        await db.rollback()
        logger.error("refresh_rotation_failed (%s)", type(exc).__name__)
        raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable") from exc
    _set_refresh_cookie(response, refresh_token)
    _set_device_session_cookie(response, raw_device_id)
    return {
        "access_token": access_token,
        "token_type": "bearer",
    }


async def _revoke_device_refresh_sessions(
    db: AsyncSession,
    device_id_hash: str,
    *,
    presented_payload: dict | None = None,
    lock_already_held: bool = False,
) -> None:
    """Revoke every family for a device plus a presented legacy family.

    Legacy rows have a NULL device hash. A signed cookie presented by login or
    logout identifies that one legacy family without inventing a backfill.
    """
    if not lock_already_held:
        await _acquire_device_session_lock(db, device_id_hash)
    filters = [RefreshSession.device_id_hash == device_id_hash]
    if presented_payload and presented_payload.get("type") == "refresh":
        try:
            family_id = uuid.UUID(str(presented_payload.get("family_id")))
        except (ValueError, TypeError, AttributeError):
            family_id = None
        if family_id is not None:
            filters.append(RefreshSession.family_id == family_id)
    await db.execute(
        update(RefreshSession)
        .where(or_(*filters))
        .values(revoked=True)
    )


async def _revoke_refresh_family(db: AsyncSession, payload: dict) -> None:
    jti = payload.get("jti")
    family_id = payload.get("family_id")
    if payload.get("type") != "refresh" or not jti or not family_id:
        raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid")
    try:
        family_uuid = uuid.UUID(str(family_id))
    except ValueError as exc:
        raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid") from exc

    subject = payload.get("sub")
    try:
        user_id = uuid.UUID(str(subject))
    except (ValueError, TypeError) as exc:
        raise _terminal_refresh_error("REFRESH_TOKEN_INVALID", "Refresh token is invalid") from exc
    locked_user = (
        await db.execute(select(User).where(User.id == user_id).with_for_update())
    ).scalar_one_or_none()
    if locked_user is None:
        raise _terminal_refresh_error(
            "REFRESH_ACCOUNT_INACTIVE",
            "Account is not active; sign in again after account access is restored",
        )

    result = await db.execute(
        select(RefreshSession)
        .where(RefreshSession.jti_hash == hash_token_identifier(jti))
        .with_for_update()
    )
    session = result.scalar_one_or_none()
    if session is None or session.user_id != user_id or session.family_id != family_uuid:
        raise _terminal_refresh_error(
            "REFRESH_SESSION_REVOKED",
            "Refresh session is not active; sign in again",
        )
    await db.execute(
        update(RefreshSession)
        .where(RefreshSession.family_id == session.family_id)
        .values(revoked=True)
    )


def _logout_error_response(status_code: int, code: str, message: str) -> JSONResponse:
    if status_code == 401 and code not in TERMINAL_REFRESH_ERROR_CODES:
        raise ValueError("Unknown terminal refresh error code")
    response = JSONResponse(
        status_code=status_code,
        content={"detail": {"code": code, "message": message}},
    )
    _clear_auth_cookies(response)
    return response


@router.post("/logout")
async def logout(request: _Request, response: Response, db: AsyncSession = Depends(get_db)):
    require_trusted_browser_origin(
        request,
        environment=settings.ENVIRONMENT,
        cookie_authenticated=bool(request.cookies.get(REFRESH_COOKIE_NAME)),
    )
    refresh_token = request.cookies.get(REFRESH_COOKIE_NAME)
    raw_device_id = request.cookies.get(DEVICE_SESSION_COOKIE_NAME)
    device_id_hash = (
        _device_session_hash(raw_device_id)
        if _valid_device_session_id(raw_device_id)
        else None
    )
    payload = None
    token_error: tuple[int, str, str] | None = None
    if refresh_token:
        try:
            payload = decode_token(refresh_token)
        except jwt.ExpiredSignatureError:
            token_error = (401, "REFRESH_TOKEN_EXPIRED", "Refresh token expired; sign in again")
        except jwt.PyJWTError:
            token_error = (401, "REFRESH_TOKEN_INVALID", "Refresh token is invalid")

    if device_id_hash is not None or payload is not None:
        try:
            if device_id_hash is not None:
                await _revoke_device_refresh_sessions(
                    db,
                    device_id_hash,
                    presented_payload=payload,
                )
            elif payload is not None:
                # Safe one-way transition for legacy clients: refresh without
                # a device cookie is rejected, but logout can still revoke the
                # signed legacy family it was given.
                await _revoke_refresh_family(db, payload)
            await db.commit()
        except HTTPException as exc:
            await db.rollback()
            detail = exc.detail if isinstance(exc.detail, dict) else {}
            code = detail.get("code", "REFRESH_SESSION_REVOKED")
            message = detail.get("message", "Refresh session is not active; sign in again")
            return _logout_error_response(exc.status_code, code, message)
        except SQLAlchemyError as exc:
            await db.rollback()
            logger.error("logout_revocation_failed (%s)", type(exc).__name__)
            return _logout_error_response(
                503,
                "REVOCATION_FAILED",
                "Logout revocation is temporarily unavailable",
            )
    if token_error is not None:
        return _logout_error_response(*token_error)
    _clear_auth_cookies(response)
    return {"message": "Logged out"}


@router.get("/stream-token")
@limiter.limit("30/minute")
async def issue_stream_token(
    request: _Request,
    current_user: Annotated[User, Depends(get_current_user)],
):
    if current_user.organization_id is None:
        raise HTTPException(status_code=403, detail="Organization membership is required for private streams")
    return {"stream_token": create_stream_token(current_user.id, current_user.organization_id)}

# ---------------------------------------------------------------------------
# Registration
# ---------------------------------------------------------------------------


async def _attribute_referral(db: AsyncSession, new_user: User, referral_code: str | None):
    """Attribute a new user to their referrer if a valid referral code is provided."""
    if not referral_code:
        return
    referrer_stmt = select(User).where(User.referral_code == referral_code)
    referrer_result = await db.execute(referrer_stmt)
    referrer = referrer_result.scalar_one_or_none()
    if referrer and referrer.id != new_user.id:
        new_user.referred_by_id = referrer.id
        db.add(Referral(
            referrer_id=referrer.id,
            referred_user_id=new_user.id,
            referral_code_used=referral_code,
        ))
        await db.commit()
        await db.refresh(new_user)


async def _assign_referral_code(db: AsyncSession, user: User):
    """Generate and assign a unique referral code to a user."""
    for _ in range(10):
        code = generate_referral_code()
        existing = await db.execute(select(User.id).where(User.referral_code == code))
        if not existing.scalar_one_or_none():
            user.referral_code = code
            return


ALLOWED_REGISTRATION_ROLES = {UserRole.BUYER, UserRole.SUPPLIER}


def _should_skip_verification_email_for_canary(request: _Request, email: str) -> bool:
    token = request.headers.get("X-Monitor-Token")
    return bool(
        settings.MONITOR_TOKEN
        and token == settings.MONITOR_TOKEN
        and is_monitor_canary_email(email)
    )


def _mask_email_for_audit(email: str) -> str:
    local, _, domain = email.partition("@")
    if not domain:
        return "***"
    if not local:
        return f"***@{domain}"
    return f"{local[0]}***@{domain}"

@router.post("/register", response_model=RegistrationResponse)
@limiter.limit("5/minute")
async def register(request: _Request, user_in: UserCreate, db: AsyncSession = Depends(get_db)):
    # Prevent privilege escalation — only BUYER/SUPPLIER allowed via self-registration
    if user_in.role and UserRole(user_in.role.value) not in ALLOWED_REGISTRATION_ROLES:
        raise HTTPException(status_code=403, detail="Invalid role for self-registration")

    normalized_email = str(user_in.email).strip().lower()

    # Check if user exists. Email identity is canonicalized at the boundary;
    # the migration also enforces this invariant in PostgreSQL.
    stmt = select(User).where(User.email == normalized_email)
    result = await db.execute(stmt)
    existing_user = result.scalar_one_or_none()

    if existing_user:
        raise HTTPException(
            status_code=400,
            detail="Email already registered"
        )

    try:
        validate_password_bytes(user_in.password)
    except ValueError as exc:
        raise HTTPException(
            status_code=422,
            detail=f"Password must be no more than {MAX_PASSWORD_BYTES} UTF-8 bytes",
        ) from exc
    hashed_pw = get_password_hash(user_in.password)

    # Public mailbox domains are never tenant boundaries.
    email_domain = registration_organization_domain(str(user_in.email))

    # Check if organization exists for this domain
    existing_org = None
    if email_domain:
        stmt_org = select(Organization).where(Organization.domain == email_domain)
        result_org = await db.execute(stmt_org)
        existing_org = result_org.scalar_one_or_none()

    if existing_org:
        role_enum = None
        if user_in.role:
            try:
                role_enum = UserRole(user_in.role.value)
            except ValueError:
                pass

        verification_token, verification_hash, verification_expires = _new_email_verification_token()
        new_user = User(
            email=normalized_email,
            password_hash=hashed_pw,
            first_name=user_in.first_name,
            last_name=user_in.last_name,
            role=role_enum,
            # A domain match is only a join request hint. It never grants
            # tenant membership before an explicit organization review.
            organization_id=None,
            status=UserStatus.PENDING,
            email_verification_token_hash=verification_hash,
            email_verification_token_expires_at=verification_expires,
        )

        db.add(new_user)
        try:
            await db.flush()
        except IntegrityError as exc:
            await db.rollback()
            raise HTTPException(
                status_code=409,
                detail={"code": "REGISTRATION_CONFLICT", "message": "Registration changed concurrently; please retry"},
            ) from exc
        join_request = OrganizationJoinRequest(user_id=new_user.id, organization_id=existing_org.id)
        db.add(join_request)
        await db.flush()
        record_initial_status(db, new_user)
        await record_audit(
            db,
            user_id=new_user.id,
            action=USER_REGISTERED,
            resource_type="user",
            resource_id=new_user.id,
            changes={
                "role": new_user.role.value if new_user.role else None,
                "organization_id": None,
                "email": str(new_user.email),
                "via": "domain_join_request",
            },
            **request_audit_context(request),
        )
        await record_audit(
            db, user_id=new_user.id, action=ORGANIZATION_JOIN_REQUESTED,
            resource_type="organization_join_request", resource_id=join_request.id,
            changes={"organization_id": str(existing_org.id)}, **request_audit_context(request),
        )
        try:
            await db.commit()
        except IntegrityError as exc:
            await db.rollback()
            raise HTTPException(
                status_code=409,
                detail={"code": "REGISTRATION_CONFLICT", "message": "Registration changed concurrently; please retry"},
            ) from exc
        await db.refresh(new_user)
        track_analytics_event(
            registration_completed_event(new_user, request=request), request=request
        )

        # Referral attribution
        await _attribute_referral(db, new_user, user_in.referral_code)

        if not _should_skip_verification_email_for_canary(request, str(new_user.email)):
            await send_verification_email(new_user.email, new_user.first_name or "there", verification_token)

        return RegistrationResponse(status="created", user=new_user)

    else:
        # Store the password hash server-side. The client receives only a
        # one-time opaque handle, never a signed payload containing secrets.
        token = _new_registration_token()
        await db.execute(
            update(PendingRegistration)
            .where(
                PendingRegistration.email == normalized_email,
                PendingRegistration.used_at.is_(None),
            )
            .values(used_at=datetime.now(UTC))
        )
        db.add(PendingRegistration(
            token_hash=token.token_hash,
            email=normalized_email,
            password_hash=hashed_pw,
            first_name=user_in.first_name,
            last_name=user_in.last_name,
            role=user_in.role,
            referral_code=user_in.referral_code,
            expires_at=token.expires_at,
        ))
        try:
            await db.commit()
        except IntegrityError as exc:
            await db.rollback()
            raise HTTPException(
                status_code=409,
                detail={"code": "REGISTRATION_CONFLICT", "message": "Registration changed concurrently; please retry"},
            ) from exc
        return RegistrationResponse(status="requires_org", registration_token=token.raw)


@router.post("/register-with-org", response_model=UserResponse)
async def register_with_org(
    http_request: _Request,
    request: RegisterWithOrgRequest,
    db: AsyncSession = Depends(get_db)
):
    token_hash = hash_token_identifier(request.registration_token)
    pending_result = await db.execute(
        select(PendingRegistration)
        .where(
            PendingRegistration.token_hash == token_hash,
            PendingRegistration.used_at.is_(None),
            PendingRegistration.expires_at > datetime.now(UTC),
        )
        .with_for_update()
    )
    pending = pending_result.scalar_one_or_none()
    if pending is None:
        raise HTTPException(status_code=400, detail="Invalid or expired registration token")

    email = pending.email.strip().lower()
    email_domain = registration_organization_domain(str(email))

    stmt_org = select(Organization).where(Organization.domain == email_domain) if email_domain else None
    result_org = await db.execute(stmt_org) if stmt_org is not None else None
    if result_org is not None and result_org.scalar_one_or_none():
         raise HTTPException(status_code=400, detail="Organization already exists for this domain. Please login or register normally.")

    new_org = Organization(
        name=request.organization.name,
        type=request.organization.type,
        domain=email_domain,
        tax_id=request.organization.tax_id,
        country_code=request.organization.country_code,
    )

    db.add(new_org)
    try:
        await db.flush()
    except IntegrityError as exc:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail={
                "code": "ORGANIZATION_DOMAIN_CONFLICT",
                "message": "An organization was created for this domain; restart registration",
            },
        ) from exc

    role_str = pending.role
    final_role = UserRole.BUYER
    if role_str:
        try:
            parsed_role = UserRole(role_str.value if hasattr(role_str, "value") else role_str)
            if parsed_role in ALLOWED_REGISTRATION_ROLES:
                final_role = parsed_role
        except ValueError:
            pass

    verification_token, verification_hash, verification_expires = _new_email_verification_token()
    new_user = User(
        email=email,
        password_hash=pending.password_hash,
        first_name=pending.first_name,
        last_name=pending.last_name,
        role=final_role,
        organization_id=None,
        status=UserStatus.PENDING,
        email_verification_token_hash=verification_hash,
        email_verification_token_expires_at=verification_expires,
    )

    db.add(new_user)
    try:
        await db.flush()
    except IntegrityError as exc:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail={"code": "REGISTRATION_CONFLICT", "message": "Registration changed concurrently; please retry"},
        ) from exc
    join_request = OrganizationJoinRequest(user_id=new_user.id, organization_id=new_org.id)
    db.add(join_request)
    await db.flush()
    pending.used_at = datetime.now(UTC)
    record_initial_status(db, new_user)
    await record_audit(
        db,
        user_id=new_user.id,
        action=USER_REGISTERED,
        resource_type="user",
        resource_id=new_user.id,
        changes={
            "role": new_user.role.value if new_user.role else None,
            "organization_id": None,
            "email": str(new_user.email),
            "via": "new_org",
        },
        **request_audit_context(http_request),
    )
    await record_audit(
        db, user_id=new_user.id, action=ORGANIZATION_JOIN_REQUESTED,
        resource_type="organization_join_request", resource_id=join_request.id,
        changes={"organization_id": str(new_org.id)}, **request_audit_context(http_request),
    )
    try:
        await db.commit()
    except IntegrityError as exc:
        await db.rollback()
        raise HTTPException(
            status_code=409,
            detail={"code": "REGISTRATION_CONFLICT", "message": "Registration changed concurrently; please retry"},
        ) from exc
    await db.refresh(new_user)
    track_analytics_event(
        organization_created_event(new_user, request=http_request), request=http_request
    )
    track_analytics_event(
        registration_completed_event(new_user, request=http_request), request=http_request
    )

    # Referral attribution
    await _attribute_referral(db, new_user, pending.referral_code)

    if not _should_skip_verification_email_for_canary(http_request, str(new_user.email)):
        await send_verification_email(new_user.email, new_user.first_name or "there", verification_token)

    return new_user

# ---------------------------------------------------------------------------
# Email verification
# ---------------------------------------------------------------------------

@router.get("/verify-email")
async def verify_email(token: str, db: AsyncSession = Depends(get_db)):
    """Verify email address using the token sent at registration."""
    token_hash = hash_token_identifier(token)
    stmt = select(User).where(User.email_verification_token_hash == token_hash)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user or user.email_verification_token_expires_at is None or user.email_verification_token_expires_at <= datetime.now(UTC):
        raise HTTPException(status_code=400, detail="Invalid or expired verification link")

    user.email_verified = True
    user.email_verification_token_hash = None
    user.email_verification_token_expires_at = None

    # Progress referral status if this user was referred
    if user.referred_by_id:
        ref_stmt = select(Referral).where(Referral.referred_user_id == user.id)
        ref_result = await db.execute(ref_stmt)
        referral = ref_result.scalar_one_or_none()
        if referral and referral.status == ReferralStatus.SIGNED_UP:
            referral.status = ReferralStatus.VERIFIED
            referral.verified_at = datetime.now(UTC)

    # Generate referral code for the newly verified user
    await _assign_referral_code(db, user)

    await db.commit()

    return {"message": "Email verified successfully", "email": user.email}


@router.post("/resend-verification-email")
@limiter.limit("3/minute")
async def resend_verification_email_public(
    request: _Request,
    body: ResendVerificationRequest,
    db: AsyncSession = Depends(get_db),
):
    """Public endpoint to resend verification email by email address. Rate limited to prevent enumeration."""
    stmt = select(User).where(User.email == str(body.email).strip().lower())
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    # Always return 200 to prevent email enumeration
    if user and not user.email_verified:
        new_token, token_hash, expires_at = _new_email_verification_token()
        user.email_verification_token_hash = token_hash
        user.email_verification_token_expires_at = expires_at
        await db.commit()
        from app.services.email import send_verification_email
        await send_verification_email(user.email, user.first_name or "there", new_token)

    return {"message": "If that email is registered and unverified, we've sent a new link."}

# ---------------------------------------------------------------------------
# Profile endpoints (merged from legacy auth.py)
# ---------------------------------------------------------------------------

@router.get("/me", response_model=UserResponse)
async def read_users_me(current_user: Annotated[User, Depends(get_current_user)]):
    return current_user

@router.put("/me", response_model=UserResponse)
async def update_users_me(
    user_update: UserUpdate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    if user_update.first_name is not None:
        current_user.first_name = user_update.first_name
    if user_update.last_name is not None:
        current_user.last_name = user_update.last_name
    if user_update.role is not None:
        if current_user.role != UserRole.ADMIN:
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Only admins can change user roles",
            )
        current_user.role = user_update.role

    await db.commit()
    await db.refresh(current_user)
    return current_user


@router.put("/me/password")
@limiter.limit("3/minute")
async def change_password(
    request: _Request,
    response: Response,
    payload: PasswordChangeRequest,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Change password. Invalidates all existing tokens via password_changed_at."""
    require_trusted_browser_origin(
        request,
        environment=settings.ENVIRONMENT,
        cookie_authenticated=bool(request.cookies.get(REFRESH_COOKIE_NAME)),
    )
    presented_device_id = request.cookies.get(DEVICE_SESSION_COOKIE_NAME)
    raw_device_id = (
        presented_device_id
        if _valid_device_session_id(presented_device_id)
        else _new_device_session_id()
    )
    device_id_hash = _device_session_hash(raw_device_id)
    await _acquire_device_session_lock(db, device_id_hash)
    locked_result = await db.execute(
        select(User).where(User.id == current_user.id).with_for_update()
    )
    locked_user = locked_result.scalar_one_or_none()
    if locked_user is None:
        raise HTTPException(status_code=401, detail="Account is no longer available")

    if not verify_password(payload.current_password, locked_user.password_hash):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Current password is incorrect",
        )

    if len(payload.new_password) < 8:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="New password must be at least 8 characters",
        )

    try:
        locked_user.password_hash = get_password_hash(payload.new_password)
    except ValueError as exc:
        raise HTTPException(
            status_code=422,
            detail=f"Password must be no more than {MAX_PASSWORD_BYTES} UTF-8 bytes",
        ) from exc
    # Tokens carry a microsecond issuance claim. Preserve the precise cutoff
    # so every token issued before this write, including same-second tokens,
    # is rejected while the newly issued pair remains usable.
    locked_user.password_changed_at = datetime.now(UTC)
    locked_user.must_change_password = False

    # Revoke every prior family before adding the one tracked successor. The
    # user row lock serializes concurrent password changes; refresh/logout
    # serialize on the affected refresh-session rows.
    access_token, refresh_token = _build_token_pair(str(locked_user.id))
    try:
        await db.execute(
            update(RefreshSession)
            .where(RefreshSession.user_id == locked_user.id)
            .values(revoked=True)
        )
        await _store_refresh_session(
            db,
            locked_user.id,
            refresh_token,
            device_id_hash=device_id_hash,
        )
        await record_audit(
            db,
            user_id=locked_user.id,
            action=USER_PASSWORD_CHANGED,
            resource_type="user",
            resource_id=locked_user.id,
            changes={"password_changed": True},
            **request_audit_context(request),
        )
        await db.commit()
    except (SQLAlchemyError, ValueError) as exc:
        await db.rollback()
        logger.error("password_change_persistence_failed (%s)", type(exc).__name__)
        raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable") from exc
    _set_refresh_cookie(response, refresh_token)
    _set_device_session_cookie(response, raw_device_id)

    return {
        "message": "Password changed successfully",
        "access_token": access_token,
    }

# ---------------------------------------------------------------------------
# Password reset (public, unauthenticated)
# ---------------------------------------------------------------------------

@router.post("/forgot-password")
@limiter.limit("3/minute")
async def forgot_password(
    request: _Request,
    body: ForgotPasswordRequest,
    db: AsyncSession = Depends(get_db),
):
    """Request a password reset link. Always returns 200 (no email enumeration)."""
    safe_response = {"message": "If an account exists, a reset link has been sent."}

    stmt = select(User).where(User.email == body.email)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user or user.status != UserStatus.APPROVED:
        await record_audit(
            db,
            user_id=user.id if user else None,
            action=USER_PASSWORD_RESET_REQUESTED,
            resource_type="user",
            resource_id=user.id if user else None,
            changes={"email_provided": _mask_email_for_audit(body.email)},
            **request_audit_context(request),
        )
        await db.commit()
        return safe_response

    # Generate token, store SHA-256 hash (never store plaintext)
    token = secrets.token_urlsafe(32)
    token_hash = hashlib.sha256(token.encode()).hexdigest()

    user.password_reset_token_hash = token_hash
    user.password_reset_expires = datetime.now(UTC) + timedelta(hours=1)
    await record_audit(
        db,
        user_id=user.id,
        action=USER_PASSWORD_RESET_REQUESTED,
        resource_type="user",
        resource_id=user.id,
        changes={"email_provided": _mask_email_for_audit(body.email)},
        **request_audit_context(request),
    )
    await db.commit()

    await send_password_reset_email(user.email, user.first_name or "there", token)

    return safe_response


@router.post("/reset-password")
@limiter.limit("5/minute")
async def reset_password(
    request: _Request,
    body: ResetPasswordRequest,
    db: AsyncSession = Depends(get_db),
):
    """Reset password using a valid token."""
    if len(body.new_password) < 8:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Password must be at least 8 characters.",
        )

    token_hash = hashlib.sha256(body.token.encode()).hexdigest()

    stmt = (
        select(User)
        .where(
            User.password_reset_token_hash == token_hash,
            User.password_reset_expires > datetime.now(UTC),
        )
        .with_for_update()
    )
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid or expired reset token.",
        )

    try:
        user.password_hash = get_password_hash(body.new_password)
    except ValueError as exc:
        raise HTTPException(
            status_code=422,
            detail=f"Password must be no more than {MAX_PASSWORD_BYTES} UTF-8 bytes",
        ) from exc
    user.password_reset_token_hash = None
    user.password_reset_expires = None
    user.password_changed_at = datetime.now(UTC)
    user.must_change_password = False
    try:
        await db.execute(
            update(RefreshSession)
            .where(RefreshSession.user_id == user.id)
            .values(revoked=True)
        )
        await record_audit(
            db,
            user_id=user.id,
            action=USER_PASSWORD_RESET_COMPLETED,
            resource_type="user",
            resource_id=user.id,
            changes={"password_reset": "completed"},
            **request_audit_context(request),
        )
        await db.commit()
    except SQLAlchemyError as exc:
        await db.rollback()
        logger.error("password_reset_persistence_failed (%s)", type(exc).__name__)
        raise HTTPException(status_code=503, detail="Authentication is temporarily unavailable") from exc

    return {"message": "Password updated. You can now sign in."}

# ---------------------------------------------------------------------------
# Admin endpoints (merged from legacy auth.py)
# ---------------------------------------------------------------------------

@router.put("/approve/{user_id}", response_model=UserResponse)
@limiter.limit("60/minute")
async def approve_user(
    request: _Request,
    user_id: uuid.UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    # Cannot use require_role here — rbac imports get_current_user from this
    # module, creating a circular import. Keep manual check instead.
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=status.HTTP_403_FORBIDDEN, detail="Forbidden")

    stmt = select(User).where(User.id == user_id).with_for_update()
    result = await db.execute(stmt)
    user_to_approve = result.scalar_one_or_none()

    if not user_to_approve:
        raise HTTPException(status_code=404, detail="User not found")

    # Guard: prevent approving another admin (no-op protection; admins are already approved).
    if user_to_approve.role == UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Admin accounts cannot be managed via this endpoint",
        )

    if not user_to_approve.email_verified:
        raise HTTPException(status_code=403, detail="User email must be verified before approval")
    # Re-approving a REJECTED user is allowed (admin error correction).
    previous_status = user_to_approve.status
    user_to_approve.status = UserStatus.APPROVED
    record_status_transition(
        db, user_to_approve, from_status=previous_status, to_status=UserStatus.APPROVED
    )
    await record_audit(
        db,
        user_id=current_user.id,
        action=ADMIN_USER_APPROVED,
        resource_type="user",
        resource_id=user_to_approve.id,
        changes={"status": {"from": previous_status.value, "to": UserStatus.APPROVED.value}},
        **request_audit_context(request),
    )
    await db.commit()
    await db.refresh(user_to_approve)
    return user_to_approve


@router.put("/organization/{organization_id}/approve")
@limiter.limit("60/minute")
async def approve_organization(
    request: _Request,
    organization_id: uuid.UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Forbidden")
    result = await db.execute(select(Organization).where(Organization.id == organization_id).with_for_update())
    organization = result.scalar_one_or_none()
    if organization is None:
        raise HTTPException(status_code=404, detail="Organization not found")
    previous = organization.verification_status
    organization.verification_status = "APPROVED"
    await record_audit(db, user_id=current_user.id, action=ADMIN_ORGANIZATION_APPROVED,
                       resource_type="organization", resource_id=organization.id,
                       changes={"verification_status": {"from": previous, "to": "APPROVED"}},
                       **request_audit_context(request))
    await db.commit()
    return {"organization_id": str(organization.id), "verification_status": organization.verification_status}


@router.put("/organization/{organization_id}/reject")
@limiter.limit("60/minute")
@retry_market_transaction()
async def reject_organization(
    request: _Request,
    organization_id: uuid.UUID,
    body: AdminDecisionBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Forbidden")
    result = await db.execute(select(Organization).where(Organization.id == organization_id).with_for_update())
    organization = result.scalar_one_or_none()
    if organization is None:
        raise HTTPException(status_code=404, detail="Organization not found")
    previous = organization.verification_status
    organization.verification_status = "REJECTED"
    user_result = await db.execute(select(User).where(User.organization_id == organization.id).with_for_update())
    members = user_result.scalars().all()
    audit_context = request_audit_context(request)
    for member in members:
        await _invalidate_membership_bound_kyc(
            db,
            user=member,
            actor_user_id=current_user.id,
            reason="organization_rejected",
            audit_context=audit_context,
        )
        member.organization_id = None
    # Market cleanup is owned by the market invalidator: cancel executable
    # state and release reservations under canonical locks in this same
    # retried transaction, then persist the participant-scoped events to the
    # durable outbox before commit.
    invalidation_events = await invalidate_organization_market_access(
        db,
        organization_id=organization.id,
        actor_user_id=current_user.id,
        reason="organization_rejected",
        reference=f"organization:{organization.id}",
    )
    await enqueue_market_events(db, invalidation_events)
    await record_audit(db, user_id=current_user.id, action=ADMIN_ORGANIZATION_REJECTED,
                       resource_type="organization", resource_id=organization.id,
                       changes={"verification_status": {"from": previous, "to": "REJECTED"},
                                "market_invalidation_events": len(invalidation_events),
                                "reason": body.reason}, **request_audit_context(request))
    await db.commit()
    return {"organization_id": str(organization.id), "verification_status": organization.verification_status}


@router.put("/reject/{user_id}")
@limiter.limit("60/minute")
async def reject_user(
    request: _Request,
    user_id: uuid.UUID,
    body: AdminDecisionBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Forbidden")
    result = await db.execute(select(User).where(User.id == user_id).with_for_update())
    target = result.scalar_one_or_none()
    if target is None or target.role == UserRole.ADMIN:
        raise HTTPException(status_code=404, detail="User not found")
    previous = target.status
    target.status = UserStatus.REJECTED
    # A rejected user is fail-closed at execution time: market mutations and
    # the matching engine re-check execution_party_is_eligible under row locks.
    # Tenant-level market cleanup is owned by the organization-rejection path.
    await record_audit(db, user_id=current_user.id, action=ADMIN_USER_REJECTED,
                       resource_type="user", resource_id=target.id,
                       changes={"status": {"from": previous.value, "to": UserStatus.REJECTED.value},
                                "reason": body.reason}, **request_audit_context(request))
    await db.commit()
    return {"user_id": str(target.id), "status": target.status.value}


def _require_admin_user(current_user: User) -> None:
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(status_code=403, detail="Forbidden")


def _join_conflict(code: str, message: str) -> HTTPException:
    return HTTPException(status_code=409, detail={"code": code, "message": message})


async def _invalidate_membership_bound_kyc(
    db: AsyncSession,
    *,
    user: User,
    actor_user_id: uuid.UUID,
    reason: str,
    audit_context: dict,
) -> bool:
    if user.kyc_organization_id is None and user.kyc_status == "PENDING":
        return False
    previous_status = user.kyc_status
    previous_organization_id = user.kyc_organization_id
    user.kyc_status = "PENDING"
    user.kyc_organization_id = None
    user.kyc_rejection_reason = None
    user.kyc_external_evidence_reference = None
    user.kyc_review_note = None
    user.kyc_reviewed_by = None
    user.kyc_reviewed_at = None
    await record_audit(
        db,
        user_id=actor_user_id,
        action=KYC_MEMBERSHIP_INVALIDATED,
        resource_type="kyc",
        resource_id=user.id,
        changes={
            "kyc_status": {"from": previous_status, "to": "PENDING"},
            "kyc_organization_id": {
                "from": str(previous_organization_id) if previous_organization_id else None,
                "to": None,
            },
            "reason": reason,
        },
        **audit_context,
    )
    return True


async def _admin_review_projection(db: AsyncSession, users: list[User]) -> list[dict]:
    user_ids = [user.id for user in users]
    joins = []
    if user_ids:
        ranked_joins = (
            select(
                OrganizationJoinRequest.id.label("join_request_id"),
                func.row_number()
                .over(
                    partition_by=OrganizationJoinRequest.user_id,
                    order_by=(
                        OrganizationJoinRequest.created_at.desc(),
                        OrganizationJoinRequest.id,
                    ),
                )
                .label("candidate_row_number"),
            )
            .where(OrganizationJoinRequest.user_id.in_(user_ids))
            .subquery()
        )
        joins = (
            await db.execute(
                select(OrganizationJoinRequest)
                .join(
                    ranked_joins,
                    ranked_joins.c.join_request_id == OrganizationJoinRequest.id,
                )
                .where(
                    ranked_joins.c.candidate_row_number
                    <= ADMIN_REVIEW_MAX_JOIN_ROWS_PER_USER
                )
                .order_by(
                    OrganizationJoinRequest.user_id,
                    OrganizationJoinRequest.created_at.desc(),
                    OrganizationJoinRequest.id,
                )
            )
        ).scalars().all()
    organization_ids = {
        organization_id
        for user in users
        for organization_id in (user.organization_id, user.kyc_organization_id)
        if organization_id is not None
    }
    organization_ids.update(join.organization_id for join in joins)
    organizations = {}
    if organization_ids:
        rows = (
            await db.execute(select(Organization).where(Organization.id.in_(organization_ids)))
        ).scalars().all()
        organizations = {row.id: row for row in rows}

    joins_by_user: dict[uuid.UUID, list[OrganizationJoinRequest]] = {}
    for join in joins:
        user_joins = joins_by_user.setdefault(join.user_id, [])
        if len(user_joins) < ADMIN_REVIEW_MAX_JOIN_ROWS_PER_USER:
            user_joins.append(join)

    def organization_payload(organization_id: uuid.UUID | None) -> dict | None:
        organization = organizations.get(organization_id)
        if organization is None:
            return None
        return {
            "id": str(organization.id),
            "name": organization.name,
            "domain": organization.domain,
            "type": organization.type.value if hasattr(organization.type, "value") else organization.type,
            "verification_status": organization.verification_status,
            "tax_id_present": bool(organization.tax_id),
            "country_code": organization.country_code,
        }

    return [
        {
            "user_id": str(user.id),
            "email": user.email,
            "email_verified": user.email_verified,
            "account_status": user.status.value if hasattr(user.status, "value") else user.status,
            "role": user.role.value if hasattr(user.role, "value") else user.role,
            "kyc_status": user.kyc_status,
            "kyc_organization_id": str(user.kyc_organization_id) if user.kyc_organization_id else None,
            "kyc_external_evidence_reference": user.kyc_external_evidence_reference,
            "kyc_review_note": user.kyc_review_note,
            "kyc_reviewed_by": str(user.kyc_reviewed_by) if user.kyc_reviewed_by else None,
            "kyc_reviewed_at": user.kyc_reviewed_at,
            "current_organization": organization_payload(user.organization_id),
            "requested_organizations": [
                {
                    "request_id": str(join.id),
                    "status": join.status.value if hasattr(join.status, "value") else join.status,
                    "created_at": join.created_at,
                    "reviewed_at": join.reviewed_at,
                    "review_note": join.review_note,
                    "organization": organization_payload(join.organization_id),
                }
                for join in joins_by_user.get(user.id, [])
            ],
            "created_at": user.created_at,
        }
        for user in users
    ]


@router.get("/admin/review-queue")
@limiter.limit("60/minute")
async def admin_review_queue(
    request: _Request,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
    limit: int = Query(default=50, ge=1, le=100),
):
    """Bounded operator queue; read-only and never changes admission or KYC."""
    _require_admin_user(current_user)
    users = (
        await db.execute(
            select(User)
            .where(
                User.role != UserRole.ADMIN,
                or_(
                    User.status == UserStatus.PENDING,
                    User.kyc_status.in_(("SUBMITTED", "REVIEW_REQUIRED", "REJECTED")),
                    User.organization_id.is_(None),
                ),
            )
            .order_by(User.created_at.asc(), User.id)
            .limit(limit)
        )
    ).scalars().all()
    return {"items": await _admin_review_projection(db, list(users)), "limit": limit}


@router.get("/admin/review-queue/{user_id}")
@limiter.limit("60/minute")
async def admin_review_detail(
    request: _Request,
    user_id: uuid.UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Read-only evidence/admission detail for one operator-selected user."""
    _require_admin_user(current_user)
    user = await db.scalar(select(User).where(User.id == user_id, User.role != UserRole.ADMIN))
    if user is None:
        raise HTTPException(status_code=404, detail="Review case not found")
    return (await _admin_review_projection(db, [user]))[0]


@router.get("/organization-joins")
@limiter.limit("60/minute")
async def list_organization_joins(
    request: _Request,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
    join_status: JoinRequestStatus | None = Query(default=None, alias="status"),
):
    """List organization membership requests for explicit admin review."""
    _require_admin_user(current_user)
    statement = select(OrganizationJoinRequest).order_by(OrganizationJoinRequest.created_at.asc())
    if join_status is not None:
        statement = statement.where(OrganizationJoinRequest.status == join_status)
    rows = (await db.execute(statement)).scalars().all()
    return [
        {
            "id": str(row.id),
            "user_id": str(row.user_id),
            "organization_id": str(row.organization_id),
            "status": row.status.value,
            "reviewed_by": str(row.reviewed_by) if row.reviewed_by else None,
            "reviewed_at": row.reviewed_at,
            "review_note": row.review_note,
            "created_at": row.created_at,
        }
        for row in rows
    ]


async def _locked_pending_join(db: AsyncSession, join_request_id: uuid.UUID) -> OrganizationJoinRequest:
    result = await db.execute(
        select(OrganizationJoinRequest)
        .where(OrganizationJoinRequest.id == join_request_id)
        .with_for_update()
    )
    join_request = result.scalar_one_or_none()
    if join_request is None:
        raise HTTPException(status_code=404, detail="Organization join request not found")
    if join_request.status != JoinRequestStatus.PENDING:
        raise _join_conflict(
            "ORGANIZATION_JOIN_ALREADY_REVIEWED",
            "Organization join request has already been reviewed",
        )
    return join_request


@router.put("/organization-joins/{join_request_id}/approve")
@limiter.limit("60/minute")
async def approve_organization_join(
    request: _Request,
    join_request_id: uuid.UUID,
    body: JoinReviewBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Approve tenant membership independently from user and organization admission."""
    _require_admin_user(current_user)
    join_request = await _locked_pending_join(db, join_request_id)
    user = (
        await db.execute(select(User).where(User.id == join_request.user_id).with_for_update())
    ).scalar_one_or_none()
    organization = (
        await db.execute(
            select(Organization).where(Organization.id == join_request.organization_id).with_for_update()
        )
    ).scalar_one_or_none()
    if user is None or organization is None:
        raise _join_conflict(
            "ORGANIZATION_JOIN_TARGET_UNAVAILABLE",
            "Organization join target is no longer available",
        )
    if user.status != UserStatus.APPROVED or not user.email_verified:
        raise _join_conflict(
            "ORGANIZATION_JOIN_USER_INELIGIBLE",
            "User admission and email verification must be complete before membership approval",
        )
    if organization.verification_status != "APPROVED":
        raise _join_conflict(
            "ORGANIZATION_JOIN_ORG_INELIGIBLE",
            "Organization verification must be complete before membership approval",
        )
    if user.organization_id is not None and user.organization_id != organization.id:
        raise _join_conflict(
            "ORGANIZATION_JOIN_MEMBERSHIP_CONFLICT",
            "User already belongs to another organization",
        )

    reviewed_at = datetime.now(UTC)
    audit_context = request_audit_context(request)
    membership_changed = False
    if user.organization_id != organization.id:
        # Membership changes fail closed at execution time: kyc_organization_id
        # is reset below and execution_party_is_eligible re-checks the exact
        # tenant binding under row locks on every market mutation.
        membership_changed = True
        await _invalidate_membership_bound_kyc(
            db,
            user=user,
            actor_user_id=current_user.id,
            reason="organization_membership_changed",
            audit_context=audit_context,
        )
    user.organization_id = organization.id
    join_request.status = JoinRequestStatus.APPROVED
    join_request.reviewed_by = current_user.id
    join_request.reviewed_at = reviewed_at
    join_request.review_note = body.review_note
    await record_audit(
        db,
        user_id=current_user.id,
        action=ORGANIZATION_JOIN_APPROVED,
        resource_type="organization_join_request",
        resource_id=join_request.id,
        changes={
            "user_id": str(user.id),
            "organization_id": str(organization.id),
            "status": {"from": JoinRequestStatus.PENDING.value, "to": JoinRequestStatus.APPROVED.value},
            "review_note": body.review_note,
            "membership_changed": membership_changed,
        },
        **audit_context,
    )
    await db.commit()
    return {"id": str(join_request.id), "status": join_request.status.value}


@router.put("/organization-joins/{join_request_id}/reject")
@limiter.limit("60/minute")
async def reject_organization_join(
    request: _Request,
    join_request_id: uuid.UUID,
    body: JoinReviewBody,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
):
    """Reject a tenant membership request without changing user or organization admission."""
    _require_admin_user(current_user)
    join_request = await _locked_pending_join(db, join_request_id)
    join_request.status = JoinRequestStatus.REJECTED
    join_request.reviewed_by = current_user.id
    join_request.reviewed_at = datetime.now(UTC)
    join_request.review_note = body.review_note
    await record_audit(
        db,
        user_id=current_user.id,
        action=ORGANIZATION_JOIN_REJECTED,
        resource_type="organization_join_request",
        resource_id=join_request.id,
        changes={
            "user_id": str(join_request.user_id),
            "organization_id": str(join_request.organization_id),
            "status": {"from": JoinRequestStatus.PENDING.value, "to": JoinRequestStatus.REJECTED.value},
            "review_note": body.review_note,
        },
        **request_audit_context(request),
    )
    await db.commit()
    return {"id": str(join_request.id), "status": join_request.status.value}

_VALID_USE_CASES = {"buyer", "supplier", "financier_other"}


class SurveySubmission(BaseModel):
    model_config = ConfigDict(extra="forbid")

    use_case: str
    referral_source: str | None = None


@router.post("/survey", status_code=status.HTTP_204_NO_CONTENT)
@limiter.limit("5/minute")
async def submit_survey(
    request: _Request,
    body: SurveySubmission,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)],
) -> None:
    """Record the authenticated user's write-once onboarding answers."""
    if body.use_case not in _VALID_USE_CASES:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"use_case must be one of: {sorted(_VALID_USE_CASES)}",
        )

    if current_user.onboarding_use_case is not None:
        return

    current_user.onboarding_use_case = body.use_case
    current_user.onboarding_referral_source = body.referral_source
    await db.commit()
