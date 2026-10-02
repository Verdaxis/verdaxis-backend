"""Role-boundary coverage for self-service profile updates."""

from uuid import uuid4

import pytest
from fastapi import HTTPException
from pydantic import ValidationError
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from app.database import Base
from app.models.user import Organization, User, UserRole, UserStatus
from app.routers.auth_simple import update_users_me
from app.schemas.user import UserCreate, UserResponse, UserUpdate


@pytest.fixture
async def profile_db():
    engine = create_async_engine("sqlite+aiosqlite://", echo=False)
    async with engine.begin() as connection:
        await connection.run_sync(
            Base.metadata.create_all,
            tables=[Organization.__table__, User.__table__],
        )
    factory = async_sessionmaker(engine, class_=AsyncSession, expire_on_commit=False)
    async with factory() as session:
        yield session
    await engine.dispose()


def _user(*, role: UserRole | None) -> User:
    return User(
        id=uuid4(),
        email=f"{uuid4()}@example.com",
        password_hash="unused",
        role=role,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
    )


def test_response_allows_legacy_null_role_while_registration_requires_role():
    user = _user(role=None)

    assert UserResponse.model_validate(user).role is None
    with pytest.raises(ValidationError):
        UserCreate(
            email="new-user@example.test",
            password="valid password 9",
            role=None,
        )


@pytest.mark.asyncio
async def test_incomplete_profile_can_assign_buyer_or_supplier_once(profile_db):
    user = _user(role=None)
    profile_db.add(user)
    await profile_db.commit()

    updated = await update_users_me(
        user_update=UserUpdate(
            first_name="Onboarded",
            last_name="Member",
            role=UserRole.BUYER,
        ),
        current_user=user,
        db=profile_db,
    )

    assert updated.role == UserRole.BUYER
    assert updated.first_name == "Onboarded"
    assert updated.last_name == "Member"


@pytest.mark.asyncio
async def test_existing_role_is_an_idempotent_no_op(profile_db):
    user = _user(role=UserRole.SUPPLIER)
    profile_db.add(user)
    await profile_db.commit()

    updated = await update_users_me(
        user_update=UserUpdate(role=UserRole.SUPPLIER),
        current_user=user,
        db=profile_db,
    )

    assert updated.role == UserRole.SUPPLIER


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("current_role", "requested_role"),
    (
        (UserRole.ADMIN, UserRole.BUYER),
        (UserRole.BUYER, UserRole.SUPPLIER),
        (None, UserRole.ADMIN),
    ),
)
async def test_profile_cannot_change_or_create_privileged_role(
    profile_db,
    current_role,
    requested_role,
):
    user = _user(role=current_role)
    profile_db.add(user)
    await profile_db.commit()

    with pytest.raises(HTTPException) as exc_info:
        await update_users_me(
            user_update=UserUpdate(
                first_name="Uncommitted",
                role=requested_role,
            ),
            current_user=user,
            db=profile_db,
        )

    assert exc_info.value.status_code == 403
    await profile_db.refresh(user)
    assert user.role == current_role
    assert user.first_name is None
