from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.ext.asyncio import AsyncSession
from sqlalchemy.future import select
from app.database import get_db
from app.models.user import User as UserModel, UserStatus, UserRole
from app.schemas.user import UserResponse, Token, UserUpdate
from app.core.auth import create_access_token, get_current_user
from typing import Annotated
from uuid import UUID

router = APIRouter()

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

@router.put("/auth/me", response_model=UserResponse)
async def update_users_me(
    user_update: UserUpdate,
    current_user: Annotated[UserModel, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    if user_update.first_name is not None:
        current_user.first_name = user_update.first_name
    if user_update.last_name is not None:
        current_user.last_name = user_update.last_name
    if user_update.role is not None:
        current_user.role = user_update.role
        
    await db.commit()
    await db.refresh(current_user)
    return current_user

@router.put("/auth/switch-role/{target_role}", response_model=Token)
async def switch_role(
    target_role: str,
    current_user: Annotated[UserModel, Depends(get_current_user)],
    db: Annotated[AsyncSession, Depends(get_db)]
):
    """
    Allows an Admin user to temporarily switch their role for testing.
    Returns a new token with the switched role.
    """
    # Only Admin can switch roles
    if current_user.role != UserRole.ADMIN:
        raise HTTPException(
            status_code=status.HTTP_403_FORBIDDEN,
            detail="Only admins can switch roles for testing",
        )
    
    # Validate target role
    target_role_upper = target_role.upper()
    if target_role_upper not in ["BUYER", "SUPPLIER", "ADMIN"]:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Invalid role. Must be BUYER, SUPPLIER, or ADMIN",
        )
    
    # Generate a new token with the switched role
    # This uses our local HS256 minting
    access_token = create_access_token(data={"sub": str(current_user.id), "role": target_role_upper})
    
    return {"access_token": access_token, "token_type": "bearer"}
