"""
RBAC dependency factory.

Usage:
    from app.middleware.rbac import require_role
    from app.models.user import UserRole

    @router.post("/endpoint")
    async def my_endpoint(
        current_user: Annotated[User, Depends(require_role(UserRole.SUPPLIER, UserRole.ADMIN))],
        ...
    ):
"""
from typing import Annotated
from fastapi import Depends, HTTPException, status
from app.models.user import User, UserRole
from app.routers.auth_simple import get_current_user


def require_role(*allowed_roles: UserRole):
    """Returns a FastAPI dependency that enforces role-based access."""
    async def role_checker(current_user: Annotated[User, Depends(get_current_user)]) -> User:
        if current_user.role not in allowed_roles:
            # Deliberately generic: enumerating the allowed roles here hands
            # an attacker a map of the privilege model (Sprint 3 item 4).
            raise HTTPException(
                status_code=status.HTTP_403_FORBIDDEN,
                detail="Forbidden",
            )
        return current_user
    return role_checker
