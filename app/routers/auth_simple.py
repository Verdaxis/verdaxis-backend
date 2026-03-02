from fastapi import Request as _Request
from app.rate_limit import limiter
from datetime import timedelta
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, status
from fastapi.security import OAuth2PasswordBearer, OAuth2PasswordRequestForm
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
import jwt
from app.database import get_db
from app.models.user import User, UserRole, UserStatus, Organization
from app.schemas.user import UserCreate, UserResponse, UserUpdate, RegistrationResponse, Token, PasswordChangeRequest
from app.schemas.organization import OrganizationCreate, OrganizationResponse
from app.core.security import (
    verify_password, get_password_hash,
    create_access_token, create_refresh_token, decode_token,
    SECRET_KEY, ALGORITHM,
)
from pydantic import BaseModel
import uuid

class RegisterWithOrgRequest(BaseModel):
    registration_token: str
    organization: OrganizationCreate

class RefreshRequest(BaseModel):
    refresh_token: str

router = APIRouter(prefix="/auth", tags=["Authentication"])

oauth2_scheme = OAuth2PasswordBearer(tokenUrl="auth/login")

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
        from datetime import datetime, UTC
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

# ---------------------------------------------------------------------------
# Login — returns access + refresh tokens
# ---------------------------------------------------------------------------

@router.post("/login")
@limiter.limit("5/minute")
async def login(request: _Request, form_data: Annotated[OAuth2PasswordRequestForm, Depends()], db: AsyncSession = Depends(get_db)):
    stmt = select(User).where(User.email == form_data.username)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()
    
    if not user or not verify_password(form_data.password, user.password_hash):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Incorrect username or password",
            headers={"WWW-Authenticate": "Bearer"},
        )
        
    if user.status != UserStatus.APPROVED:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"Account is {user.status.value}. Please wait for admin approval.",
        )
    
    # Update last_login
    from datetime import datetime, UTC
    user.last_login = datetime.now(UTC)
    await db.commit()
    
    access_token = create_access_token(
        subject=str(user.id),
        additional_claims={"role": user.role.value if user.role else None},
    )
    refresh_token = create_refresh_token(subject=str(user.id))
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": "bearer",
    }

# ---------------------------------------------------------------------------
# Refresh — exchange refresh token for new access + refresh tokens
# ---------------------------------------------------------------------------

@router.post("/refresh")
@limiter.limit("10/minute")
async def refresh_tokens(request: _Request, body: RefreshRequest, db: AsyncSession = Depends(get_db)):
    try:
        payload = decode_token(body.refresh_token)
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
        from datetime import datetime, UTC
        iat_dt = datetime.fromtimestamp(token_iat, tz=UTC)
        if iat_dt < user.password_changed_at:
            raise HTTPException(status_code=401, detail="Password was changed. Please log in again.")

    access_token = create_access_token(
        subject=str(user.id),
        additional_claims={"role": user.role.value if user.role else None},
    )
    refresh_token = create_refresh_token(subject=str(user.id))
    return {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "token_type": "bearer",
    }

# ---------------------------------------------------------------------------
# Registration
# ---------------------------------------------------------------------------

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

        new_user = User(
            email=user_in.email,
            password_hash=hashed_pw,
            first_name=user_in.first_name,
            last_name=user_in.last_name,
            role=role_enum,
            organization_id=existing_org.id,
            status=UserStatus.PENDING
        )
        
        db.add(new_user)
        await db.commit()
        await db.refresh(new_user)
        return RegistrationResponse(status="created", user=new_user)
    
    else:
        # Defer flow: Return token for org creation
        token_data = {
            "email": user_in.email,
            "password_hash": hashed_pw,
            "first_name": user_in.first_name,
            "last_name": user_in.last_name,
            "role": user_in.role.value if user_in.role else None,
            "type": "registration"
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

    new_user = User(
        email=email,
        password_hash=payload.get("password_hash"),
        first_name=payload.get("first_name"),
        last_name=payload.get("last_name"),
        role=final_role, 
        organization_id=new_org.id,
        status=UserStatus.PENDING
    )
    
    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)
    
    return new_user

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
async def change_password(
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
    
    from datetime import datetime, UTC
    current_user.password_hash = get_password_hash(payload.new_password)
    current_user.password_changed_at = datetime.now(UTC)
    
    await db.commit()
    
    # Return fresh tokens so the user stays logged in
    access_token = create_access_token(
        subject=str(current_user.id),
        additional_claims={"role": current_user.role.value if current_user.role else None},
    )
    refresh_token = create_refresh_token(subject=str(current_user.id))
    
    return {
        "message": "Password changed successfully",
        "access_token": access_token,
        "refresh_token": refresh_token,
    }

# ---------------------------------------------------------------------------
# Admin endpoints (merged from legacy auth.py)
# ---------------------------------------------------------------------------

@router.put("/approve/{user_id}", response_model=UserResponse)
async def approve_user(
    user_id: uuid.UUID,
    current_user: Annotated[User, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Not authorized to approve users",
        )
        
    stmt = select(User).where(User.id == user_id)
    result = await db.execute(stmt)
    user_to_approve = result.scalar_one_or_none()
    
    if not user_to_approve:
        raise HTTPException(status_code=404, detail="User not found")
        
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
