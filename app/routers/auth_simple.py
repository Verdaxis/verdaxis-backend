from datetime import timedelta
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, status
from fastapi.security import OAuth2PasswordBearer, OAuth2PasswordRequestForm
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from jose import JWTError, jwt
from app.database import get_db
from app.models.user import User, UserRole, UserStatus, Organization
from app.schemas.user import UserCreate, UserResponse, UserUpdate, RegistrationResponse
from app.schemas.organization import OrganizationCreate, OrganizationResponse
from app.core.security import verify_password, get_password_hash, create_access_token, SECRET_KEY, ALGORITHM
from pydantic import BaseModel
import uuid
import re
from jose import jwt, JWTError

class RegisterWithOrgRequest(BaseModel):
    registration_token: str
    organization: OrganizationCreate

router = APIRouter(prefix="/auth", tags=["Authentication"])

oauth2_scheme = OAuth2PasswordBearer(tokenUrl="auth/login")

async def get_current_user(token: Annotated[str, Depends(oauth2_scheme)], db: AsyncSession = Depends(get_db)):
    credentials_exception = HTTPException(
        status_code=status.HTTP_401_UNAUTHORIZED,
        detail="Could not validate credentials",
        headers={"WWW-Authenticate": "Bearer"},
    )
    try:
        payload = jwt.decode(token, SECRET_KEY, algorithms=[ALGORITHM])
        user_id_str: str = payload.get("sub")
        if user_id_str is None:
            raise credentials_exception
    except JWTError:
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
    return user

@router.post("/login")
async def login(form_data: Annotated[OAuth2PasswordRequestForm, Depends()], db: AsyncSession = Depends(get_db)):
    # Authenticate
    stmt = select(User).where(User.email == form_data.username) # OAuth2 form uses 'username' field
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
    
    access_token = create_access_token(subject=str(user.id))
    return {"access_token": access_token, "token_type": "bearer"}

@router.post("/register", response_model=RegistrationResponse)
async def register(user_in: UserCreate, db: AsyncSession = Depends(get_db)):
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
        # Helper to convert to Model Enum
        role_enum = None
        if user_in.role:
            try:
                # Ensure we use the imported UserRole from models
                # user_in.role is a string-based enum from Schema, so .value gives the string
                role_enum = UserRole(user_in.role.value)
            except ValueError:
                pass

        # Normal flow: Create User linked to Org
        new_user = User(
            email=user_in.email,
            password_hash=hashed_pw,
            first_name=user_in.first_name,
            last_name=user_in.last_name,
            role=role_enum,
            organization_id=existing_org.id,
            status=UserStatus.APPROVED
        )
        
        db.add(new_user)
        await db.commit()
        await db.refresh(new_user)
        return RegistrationResponse(status="created", user=new_user)
    
    else:
        # Defer flow: Return token
        # Encode user details in token
        token_data = {
            "email": user_in.email,
            "password_hash": hashed_pw,
            "first_name": user_in.first_name,
            "last_name": user_in.last_name,
            "role": user_in.role.value if user_in.role else None,
            "type": "registration"
        }
        # Short expiry for registration token (e.g., 30 mins)
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
    # Verify Token
    try:
        payload = jwt.decode(request.registration_token, SECRET_KEY, algorithms=[ALGORITHM])
        if payload.get("type") != "registration":
             raise HTTPException(status_code=400, detail="Invalid token type")
    except JWTError:
        raise HTTPException(status_code=400, detail="Invalid or expired registration token")
        
    email = payload.get("email")
    email_domain = email.split('@')[1]
    
    # Double check if org was created in the meantime
    stmt_org = select(Organization).where(Organization.domain == email_domain)
    result_org = await db.execute(stmt_org)
    if result_org.scalar_one_or_none():
         raise HTTPException(status_code=400, detail="Organization already exists for this domain. Please login or register normally.")

    # Create Organization
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
    
    # Create User
    # Create User
    role_str = payload.get("role")
    final_role = UserRole.ADMIN # Default fallback
    if role_str:
        try:
             final_role = UserRole(role_str)
        except ValueError:
             pass

    new_user = User(
        email=email,
        password_hash=payload.get("password_hash"),
        first_name=payload.get("first_name"),
        last_name=payload.get("last_name"),
        role=final_role, 
        organization_id=new_org.id,
        status=UserStatus.APPROVED # Or APPROVED? Keeping PENDING by default
    )
    
    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)
    
    return new_user



@router.get("/me", response_model=UserResponse)
async def read_users_me(current_user: Annotated[User, Depends(get_current_user)]):
    return current_user
