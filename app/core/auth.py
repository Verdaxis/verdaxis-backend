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
oauth2_scheme = OAuth2PasswordBearer(tokenUrl=f"{settings.AUTHENTIK_DOMAIN}/application/o/authorize/")

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

async def get_oidc_config() -> Dict[str, Any]:
    """
    Fetches the OpenID Connect configuration from Authentik.
    """
    try:
        async with httpx.AsyncClient() as client:
            resp = await client.get(f"{settings.AUTHENTIK_DOMAIN}/application/o/verdaxis/.well-known/openid-configuration")
            resp.raise_for_status()
            return resp.json()
    except Exception as e:
        print(f"Error fetching OIDC config: {e}")
        # Fallback or retry implementation would go here
        return {}

async def get_jwks() -> Dict[str, Any]:
    """
    Fetches the JSON Web Key Set (KWKS) for signature verification.
    """
    try:
        oidc_config = await get_oidc_config()
        jwks_uri = oidc_config.get("jwks_uri")
        if not jwks_uri:
            # Fallback construction if OIDC config fails
            jwks_uri = f"{settings.AUTHENTIK_DOMAIN}/application/o/verdaxis/jwks/"
            
        async with httpx.AsyncClient() as client:
            resp = await client.get(jwks_uri)
            resp.raise_for_status()
            return resp.json()
    except Exception as e:
        print(f"Error fetching JWKS: {e}")
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Could not verify authentication configuration"
        )

async def verify_token(token: str) -> Dict[str, Any]:
    """
    Verifies the JWT token against Authentik's public keys.
    """
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    
    try:
        # 1. Try Legacy/Local HS256 Token (for switch_role)
        # We attempt this if the header alg is HS256
        unverified_header = jwt.get_unverified_header(token)
        if unverified_header.get("alg") == "HS256":
             from app.config import settings # re-import to be safe
             # We need a secret for local tokens. We'll use a fallback or the one from generic config
             local_secret = "dev-secret-key-not-for-production" # Hardcoded backup or env
             if hasattr(settings, "JWT_SECRET"):
                 local_secret = settings.JWT_SECRET
             
             return jwt.decode(token, local_secret, algorithms=["HS256"])

        # 2. Try Authentik RS256 Token
        jwks = await get_jwks()
        
        # Decode and verify
        payload = jwt.decode(
            token,
            jwks,
            algorithms=["RS256"],
            audience=settings.AUTHENTIK_CLIENT_ID,
            options={"verify_at_hash": False}
        )
        return payload
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
    # 0. Check for Dev Bypass
    if settings.ENABLE_AUTH_BYPASS:
        # Return a mock Dev Admin user
        # We need to ensure this user exists in the DB so that relationships work
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

    payload = await verify_token(token)
    email: str = payload.get("email")
    if email is None:
        raise HTTPException(status_code=401, detail="Token missing email claim")

    # 1. Check if user exists locally
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
            role=UserRole.BUYER, # Default role
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
