from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.user import User as UserModel
from app.schemas.user import LoginRequest, Token, UserResponse
from app.core.security import verify_password, get_password_hash
from app.core.auth import create_access_token, get_current_user
from typing import Annotated
from app.models.user import UserStatus, UserRole
from app.schemas.user import UserCreate
from uuid import UUID

router = APIRouter()


@router.post("/auth/login", response_model=Token)
async def login(
    login_data: LoginRequest,
    db: Annotated[AsyncSession, Depends(get_db)]
):
    # Find user by email
    stmt = select(UserModel).where(UserModel.email == login_data.email)
    result = await db.execute(stmt)
    user = result.scalar_one_or_none()
    
    if not user:
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Incorrect email or password",
        )
    
    # Verify password
    if not verify_password(login_data.password, user.password_hash):
        raise HTTPException(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail="Incorrect email or password",
        )

    # Check verification status
    if user.status != UserStatus.APPROVED:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=f"Account is {user.status.value}. Please wait for admin approval.",
        )
        
    # Generate Token
    access_token = create_access_token(data={"sub": str(user.id), "role": user.role.value})
    
    return {"access_token": access_token, "token_type": "bearer"}

@router.post("/auth/register", response_model=UserResponse)
async def register(
    user_data: UserCreate,
    db: Annotated[AsyncSession, Depends(get_db)]
):
    # Check if user already exists
    stmt = select(UserModel).where(UserModel.email == user_data.email)
    result = await db.execute(stmt)
    existing_user = result.scalar_one_or_none()
    
    if existing_user:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Email already registered",
        )
    
    # Create new user
    hashed_password = get_password_hash(user_data.password)
    new_user = UserModel(
        email=user_data.email,
        password_hash=hashed_password,
        first_name=user_data.first_name,
        last_name=user_data.last_name,
        role=user_data.role,
        organization_id=user_data.organization_id,
        status=UserStatus.PENDING # Default to PENDING
    )
    
    db.add(new_user)
    await db.commit()
    await db.refresh(new_user)
    
    return new_user

@router.put("/auth/approve/{user_id}", response_model=UserResponse)
async def approve_user(
    user_id: UUID,
    current_user: Annotated[UserModel, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    # Only Admin can approve
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Not authorized to approve users",
        )
        
    stmt = select(UserModel).where(UserModel.id == user_id)
    result = await db.execute(stmt)
    user_to_approve = result.scalar_one_or_none()
    
    if not user_to_approve:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="User not found",
        )
        
    user_to_approve.status = UserStatus.APPROVED
    await db.commit()
    await db.refresh(user_to_approve)
    
    return user_to_approve

@router.get("/auth/me", response_model=UserResponse)
async def read_users_me(current_user: Annotated[UserModel, Depends(get_current_user)]):
    return current_user

