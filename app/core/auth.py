from typing import Optional, Dict, Any
from fastapi import Depends, HTTPException, status
from fastapi.security import OAuth2PasswordBearer
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from jose import jwt, JWTError
import httpx

from app.database import get_db
from app.config import settings
from app.models.user import User, UserRole, UserStatus

# Defines the token source - frontend will send "Authorization: Bearer <token>"
oauth2_scheme = OAuth2PasswordBearer(tokenUrl=f"{settings.API_V1_STR}/auth/login")

def create_access_token(data: dict, expires_delta: Optional[Any] = None):
    # Local HS256 Token Generation (for switch_role)
    from datetime import datetime, timedelta
    to_encode = data.copy()
    if expires_delta:
        expire = datetime.utcnow() + expires_delta
    else:
        expire = datetime.utcnow() + timedelta(minutes=60*24)
    
    to_encode.update({"exp": expire})
    # Use the local secret
    encoded_jwt = jwt.encode(to_encode, settings.JWT_SECRET, algorithm="HS256")
    return encoded_jwt

async def verify_token(token: str) -> Dict[str, Any]:
    """
    Verifies the JWT token using local HS256.
    """
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    
    try:
        # Simply decode using HS256 and local secret
        return jwt.decode(token, settings.JWT_SECRET, algorithms=["HS256"])
    except JWTError as e:
        print(f"JWT Verification Error: {e}")
        raise credentials_exception

async def get_current_user(
    token: str = Depends(oauth2_scheme), 
    db: AsyncSession = Depends(get_db)
) -> User:
    """
    Validates token and returns the user.
    Provisions the user in the local DB if they don't exist (JIT).
    """
    # 0. Validate Token if present
    payload = None
    if token and token != "undefined" and token != "null":
        try:
            payload = await verify_token(token)
        except HTTPException:
            if not settings.ENABLE_AUTH_BYPASS:
                raise

    # 1. Check for Dev Bypass if token is missing or invalid
    if not payload and settings.ENABLE_AUTH_BYPASS:
        # Return a mock Dev Admin user
        email = "dev@admin.com"
        stmt = select(User).where(User.email == email)
        result = await db.execute(stmt)
        user = result.scalar_one_or_none()
        
        if user is None:
            user = User(
                email=email,
                first_name="Dev",
                last_name="Admin",
                password_hash="bypass_managed",
                role=UserRole.ADMIN,
                status=UserStatus.APPROVED
            )
            db.add(user)
            await db.commit()
            await db.refresh(user)
        return user
    
    if not payload:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Token missing or invalid",
            headers={"WWW-Authenticate": "Bearer"},
        )

    email: str = payload.get("email") or payload.get("sub") # auth_simple uses 'sub' for user_id/email
    if email is None:
        raise HTTPException(status_code=401, detail="Token missing email/sub claim")
    
    # Check if sub is a UUID (from auth_simple) or an email
    user_id = None
    try:
        import uuid
        user_id = uuid.UUID(email)
    except (ValueError, TypeError):
        pass

    # 1. Check if user exists locally
    if user_id:
        stmt = select(User).where(User.id == user_id)
    else:
        stmt = select(User).where(User.email == email)
        
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()

    # 2. JIT Provisioning
    if user is None:
        # Extract details from token claims
        first_name = payload.get("given_name", "")
        last_name = payload.get("family_name", "")
        
        # For security, new JIT users are PENDING by default
        # You might auto-approve if they match a specific domain
        new_user = User(
            email=email,
            first_name=first_name,
            last_name=last_name,
            password_hash="sso_managed", # Placeholder, they don't use password here
            role=None, # Default to None to trigger Onboarding
            status=UserStatus.PENDING # Must be approved by Admin
        )
        db.add(new_user)
        await db.commit()
        await db.refresh(new_user)
        user = new_user

    # 3. Check Account Status
    if user.status != UserStatus.APPROVED:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"Account is {user.status.value}. Please wait for admin approval.",
        )

    return user
