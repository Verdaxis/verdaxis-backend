from datetime import timedelta
from typing import Annotated
from fastapi import APIRouter, Depends, HTTPException, status
from fastapi.security import OAuth2PasswordBearer, OAuth2PasswordRequestForm
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy import select
from jose import JWTError, jwt
from app.database import get_db
from app.models.user import User, UserRole, UserStatus, Organization
from app.schemas.user import UserCreate, UserResponse, UserUpdate
from app.schemas.organization import OrganizationCreate, OrganizationResponse
from app.core.security import verify_password, get_password_hash, create_access_token, SECRET_KEY, ALGORITHM
import uuid
import re

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
    
    access_token = create_access_token(subject=str(user.id))
    return {"access_token": access_token, "token_type": "bearer"}

@router.post("/register", response_model=UserResponse)
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
    
    final_org_id = None
    if existing_org:
        final_org_id = existing_org.id
    
    new_user = User(
        email=user_in.email,
        password_hash=hashed_pw,
        first_name=user_in.first_name,
        last_name=user_in.last_name,
        role=user_in.role if user_in.role else None, # Allow null initially
        organization_id=final_org_id,
        status=UserStatus.PENDING
    )
    
    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)
    return new_user

@router.post("/create-organization", response_model=UserResponse)
async def create_organization(
    org_in: OrganizationCreate,
    current_user: Annotated[User, Depends(get_current_user)],
    db: AsyncSession = Depends(get_db)
):
    if current_user.organization_id:
        raise HTTPException(
            status_code=400,
            detail="User already belongs to an organization"
        )
        
    email_domain = current_user.email.split('@')[1]
    
    # Check if domain is blacklisted or public (optional, skipping for now as per req)
    
    # Create Organization
    new_org = Organization(
        name=org_in.name,
        type=org_in.type,
        domain=email_domain,
        tax_id=org_in.tax_id,
        country_code=org_in.country_code,
        verification_status="PENDING"
    )
    
    db.add(new_org)
    await db.flush() # Get ID
    
    # Update User
    current_user.organization_id = new_org.id
    current_user.role = UserRole.ADMIN # First user is Admin
    
    # We might want to auto-approve them if they created the org? 
    # Or keep them PENDING until platform admin approves the org? 
    # For now, let's keep status as is or set to APPROVED if that's the flow.
    # Requirement doesn't specify approval, but "User signs up... lead to Org Creation".
    # Let's assume joining an org makes you a member. Creating it makes you an Admin of it.
    
    await db.commit()
    await db.refresh(current_user)
    
    return current_user

@router.get("/me", response_model=UserResponse)
async def read_users_me(current_user: Annotated[User, Depends(get_current_user)]):
    return current_user
