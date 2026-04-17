from fastapi import Request as _Request
from app.rate_limit import limiter
from datetime import datetime, timedelta, UTC
import os
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, Response, status
from fastapi.security import OAuth2PasswordBearer, OAuth2PasswordRequestForm
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
import hashlib
import jwt
import secrets
from app.database import get_db
from app.models.user import User, UserRole, UserStatus, Organization
from app.schemas.user import UserCreate, UserResponse, UserUpdate, RegistrationResponse, Token, PasswordChangeRequest
from app.schemas.organization import OrganizationCreate
from app.core.security import (
    verify_password, get_password_hash,
    create_access_token, create_refresh_token, decode_token,
    REFRESH_TOKEN_EXPIRE_DAYS,
)
from pydantic import BaseModel
import uuid

from app.models.referral import Referral, ReferralStatus, generate_referral_code
from app.services.email import send_verification_email, send_password_reset_email

class RegisterWithOrgRequest(BaseModel):
    registration_token: str
    organization: OrganizationCreate

class ResendVerificationRequest(BaseModel):
    email: str

class RefreshRequest(BaseModel):
    refresh_token: str | None = None

class ForgotPasswordRequest(BaseModel):
    email: str

class ResetPasswordRequest(BaseModel):
    token: str
    new_password: str

router = APIRouter(prefix="/auth", tags=["Authentication"])

REFRESH_COOKIE_NAME = "refresh_token"
REFRESH_COOKIE_PATH = "/api/auth"
REFRESH_COOKIE_MAX_AGE = REFRESH_TOKEN_EXPIRE_DAYS * 24 * 60 * 60
REFRESH_COOKIE_SECURE = os.getenv("ENVIRONMENT", "production").lower() == "production"
REFRESH_COOKIE_SAMESITE = "lax"

oauth2_scheme = OAuth2PasswordBearer(tokenUrl="auth/login")


def _build_token_pair(subject: str, role: UserRole | None = None) -> tuple[str, str]:
    access_token = create_access_token(
        subject=subject,
        additional_claims={"role": role.value if role else None},
    )
    refresh_token = create_refresh_token(subject=subject)
    return access_token, refresh_token


def _set_refresh_cookie(response: Response, refresh_token: str) -> None:
    response.set_cookie(
        key=REFRESH_COOKIE_NAME,
        value=refresh_token,
        httponly=True,
        secure=REFRESH_COOKIE_SECURE,
        samesite=REFRESH_COOKIE_SAMESITE,
        max_age=REFRESH_COOKIE_MAX_AGE,
        path=REFRESH_COOKIE_PATH,
    )


def _clear_refresh_cookie(response: Response) -> None:
    response.delete_cookie(
        key=REFRESH_COOKIE_NAME,
        path=REFRESH_COOKIE_PATH,
    )

async def get_current_user(token: Annotated[str, Depends(oauth2_scheme)], db: AsyncSession = Depends(get_db)):
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    try:
        payload = decode_token(token)
        # Reject refresh tokens used as access tokens
        if payload.get("type") == "refresh":
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

    stmt = select(User).where(User.id == user_uuid)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if user is None:
        raise credentials_exception

    # Stateless password invalidation: reject tokens issued before password change
    if user.password_changed_at is not None:
        token_iat = payload.get("iat")
        if token_iat is None:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Token missing issued-at claim. Please log in again.",
                headers={"WWW-Authenticate": "Bearer"},
            )
        iat_dt = datetime.fromtimestamp(token_iat, tz=UTC)
        if iat_dt < user.password_changed_at:
            raise HTTPException(
                status_code=status.HTTP_401_UNAUTHORIZED,
                detail="Password was changed. Please log in again.",
                headers={"WWW-Authenticate": "Bearer"},
            )

    if user.status != UserStatus.APPROVED:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"Account is {user.status.value}. Please wait for admin approval.",
        )

    return user

async def get_current_user_optional(
    request: _Request,
    db: AsyncSession = Depends(get_db),
) -> 'User | None':
    """Like get_current_user but returns None when no/invalid token is present."""
    from fastapi.security.utils import get_authorization_scheme_param
    authorization = request.headers.get('Authorization', '')
    scheme, token = get_authorization_scheme_param(authorization)
    if not token or scheme.lower() != 'bearer':
        return None
    try:
        return await get_current_user(token=token, db=db)
    except Exception:
        return None


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
    stmt = select(User).where(User.email == form_data.username)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user or not verify_password(form_data.password, user.password_hash):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Incorrect username or password",
            headers={"WWW-Authenticate": "Bearer"},
        )

    # Email must be verified before login is permitted
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

    # Update last_login
    user.last_login = datetime.now(UTC)
    await db.commit()

    access_token, refresh_token = _build_token_pair(str(user.id), user.role)
    _set_refresh_cookie(response, refresh_token)
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
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
    refresh_token = None
    if body and body.refresh_token:
        refresh_token = body.refresh_token
    else:
        refresh_token = request.cookies.get(REFRESH_COOKIE_NAME)

    if not refresh_token:
        raise HTTPException(status_code=401, detail="Missing refresh token")

    try:
        payload = decode_token(refresh_token)
        if payload.get("type") != "refresh":
            raise HTTPException(status_code=401, detail="Invalid token type")
        user_id_str = payload.get("sub")
        if not user_id_str:
            raise HTTPException(status_code=401, detail="Invalid token")
    except jwt.ExpiredSignatureError:
        raise HTTPException(status_code=401, detail="Refresh token expired. Please log in again.")
    except jwt.PyJWTError:
        raise HTTPException(status_code=401, detail="Invalid refresh token")

    try:
        user_uuid = uuid.UUID(user_id_str)
    except ValueError:
        raise HTTPException(status_code=401, detail="Invalid token subject")

    stmt = select(User).where(User.id == user_uuid)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user or user.status != UserStatus.APPROVED:
        raise HTTPException(status_code=401, detail="User not found or not active")

    # Check password_changed_at invalidation on refresh too
    if user.password_changed_at is not None:
        token_iat = payload.get("iat")
        if token_iat is None:
            raise HTTPException(status_code=401, detail="Token missing issued-at claim. Please log in again.")
        iat_dt = datetime.fromtimestamp(token_iat, tz=UTC)
        if iat_dt < user.password_changed_at:
            raise HTTPException(status_code=401, detail="Password was changed. Please log in again.")

    access_token, refresh_token = _build_token_pair(str(user.id), user.role)
    _set_refresh_cookie(response, refresh_token)
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": "bearer",
    }


@router.post("/logout")
async def logout(response: Response):
    _clear_refresh_cookie(response)
    return {"message": "Logged out"}

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

@router.post("/register", response_model=RegistrationResponse)
@limiter.limit("5/minute")
async def register(request: _Request, user_in: UserCreate, db: AsyncSession = Depends(get_db)):
    # Prevent privilege escalation — only BUYER/SUPPLIER allowed via self-registration
    if user_in.role and UserRole(user_in.role.value) not in ALLOWED_REGISTRATION_ROLES:
        raise HTTPException(status_code=403, detail="Invalid role for self-registration")

    # Check if user exists
    stmt = select(User).where(User.email == user_in.email)
    result = await db.execute(stmt)
    existing_user = result.scalar_one_or_none()

    if existing_user:
        raise HTTPException(
            status_code=400,
            detail="Email already registered"
        )

    hashed_pw = get_password_hash(user_in.password)

    # Extract domain from email
    email_domain = user_in.email.split('@')[1]

    # Check if organization exists for this domain
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

        verification_token = secrets.token_urlsafe(32)
        new_user = User(
            email=user_in.email,
            password_hash=hashed_pw,
            first_name=user_in.first_name,
            last_name=user_in.last_name,
            role=role_enum,
            organization_id=existing_org.id,
            status=UserStatus.PENDING,
            email_verification_token=verification_token,
        )

        db.add(new_user)
        await db.commit()
        await db.refresh(new_user)

        # Referral attribution
        await _attribute_referral(db, new_user, user_in.referral_code)

        await send_verification_email(new_user.email, new_user.first_name or "there", verification_token)

        return RegistrationResponse(status="created", user=new_user)

    else:
        # Defer flow: Return token for org creation
        token_data = {
            "email": user_in.email,
            "password_hash": hashed_pw,
            "first_name": user_in.first_name,
            "last_name": user_in.last_name,
            "role": user_in.role.value if user_in.role else None,
            "type": "registration",
            "referral_code": user_in.referral_code,
        }
        reg_token = create_access_token(
            subject=user_in.email,
            expires_delta=timedelta(minutes=30),
            additional_claims=token_data
        )

        return RegistrationResponse(status="requires_org", registration_token=reg_token)


@router.post("/register-with-org", response_model=UserResponse)
async def register_with_org(
    request: RegisterWithOrgRequest,
    db: AsyncSession = Depends(get_db)
):
    try:
        payload = decode_token(request.registration_token)
        if payload.get("type") != "registration":
             raise HTTPException(status_code=400, detail="Invalid token type")
    except jwt.PyJWTError:
        raise HTTPException(status_code=400, detail="Invalid or expired registration token")

    email = payload.get("email")
    email_domain = email.split('@')[1]

    stmt_org = select(Organization).where(Organization.domain == email_domain)
    result_org = await db.execute(stmt_org)
    if result_org.scalar_one_or_none():
         raise HTTPException(status_code=400, detail="Organization already exists for this domain. Please login or register normally.")

    new_org = Organization(
        name=request.organization.name,
        type=request.organization.type,
        domain=email_domain,
        tax_id=request.organization.tax_id,
        country_code=request.organization.country_code,
        verification_status="PENDING"
    )

    db.add(new_org)
    await db.flush()

    role_str = payload.get("role")
    final_role = UserRole.BUYER
    if role_str:
        try:
            parsed_role = UserRole(role_str)
            if parsed_role in ALLOWED_REGISTRATION_ROLES:
                final_role = parsed_role
        except ValueError:
            pass

    verification_token = secrets.token_urlsafe(32)
    new_user = User(
        email=email,
        password_hash=payload.get("password_hash"),
        first_name=payload.get("first_name"),
        last_name=payload.get("last_name"),
        role=final_role,
        organization_id=new_org.id,
        status=UserStatus.PENDING,
        email_verification_token=verification_token,
    )

    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)

    # Referral attribution
    await _attribute_referral(db, new_user, payload.get("referral_code"))

    await send_verification_email(new_user.email, new_user.first_name or "there", verification_token)

    return new_user

# ---------------------------------------------------------------------------
# Email verification
# ---------------------------------------------------------------------------

@router.get("/verify-email")
async def verify_email(token: str, db: AsyncSession = Depends(get_db)):
    """Verify email address using the token sent at registration."""
    stmt = select(User).where(User.email_verification_token == token)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user:
        raise HTTPException(status_code=400, detail="Invalid or expired verification link")

    user.email_verified = True
    user.email_verification_token = None
    user.status = UserStatus.APPROVED

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


@router.post("/resend-verification")
async def resend_verification(
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db),
):
    """Resend the email verification link for the currently authenticated user."""
    if current_user.email_verified:
        raise HTTPException(status_code=400, detail="Email already verified")

    new_token = secrets.token_urlsafe(32)
    current_user.email_verification_token = new_token
    await db.commit()

    await send_verification_email(current_user.email, current_user.first_name or "there", new_token)

    return {"message": "Verification email sent"}


@router.post("/resend-verification-email")
@limiter.limit("3/minute")
async def resend_verification_email_public(
    request: _Request,
    body: ResendVerificationRequest,
    db: AsyncSession = Depends(get_db),
):
    """Public endpoint to resend verification email by email address. Rate limited to prevent enumeration."""
    stmt = select(User).where(User.email == body.email)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    # Always return 200 to prevent email enumeration
    if user and not user.email_verified:
        new_token = secrets.token_urlsafe(32)
        user.email_verification_token = new_token
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
    if not verify_password(payload.current_password, current_user.password_hash):
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Current password is incorrect",
        )

    if len(payload.new_password) < 8:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="New password must be at least 8 characters",
        )

    current_user.password_hash = get_password_hash(payload.new_password)
    current_user.password_changed_at = datetime.now(UTC)

    await db.commit()

    # Return fresh tokens so the user stays logged in
    access_token, refresh_token = _build_token_pair(str(current_user.id), current_user.role)
    _set_refresh_cookie(response, refresh_token)

    return {
        "message": "Password changed successfully",
        "access_token": access_token,
        "refresh_token": refresh_token,
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
        return safe_response

    # Generate token, store SHA-256 hash (never store plaintext)
    token = secrets.token_urlsafe(32)
    token_hash = hashlib.sha256(token.encode()).hexdigest()

    user.password_reset_token_hash = token_hash
    user.password_reset_expires = datetime.now(UTC) + timedelta(hours=1)
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

    stmt = select(User).where(
        User.password_reset_token_hash == token_hash,
        User.password_reset_expires > datetime.now(UTC),
    )
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    if not user:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid or expired reset token.",
        )

    user.password_hash = get_password_hash(body.new_password)
    user.password_reset_token_hash = None
    user.password_reset_expires = None
    user.password_changed_at = datetime.now(UTC)
    await db.commit()

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

    stmt = select(User).where(User.id == user_id)
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

    # Re-approving a REJECTED user is allowed (admin error correction).
    user_to_approve.status = UserStatus.APPROVED
    await db.commit()
    await db.refresh(user_to_approve)
    return user_to_approve

@router.put("/switch-role/{target_role}", response_model=Token)
async def switch_role(
    target_role: str,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    """Allows an Admin user to temporarily switch their role for testing."""
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only admins can switch roles for testing",
        )

    target_role_upper = target_role.upper()
    if target_role_upper not in ["BUYER", "SUPPLIER", "ADMIN"]:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid role. Must be BUYER, SUPPLIER, or ADMIN",
        )

    access_token = create_access_token(
        subject=str(current_user.id),
        additional_claims={"role": target_role_upper},
    )
    return {"access_token": access_token, "token_type": "bearer"}
