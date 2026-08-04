from datetime import UTC, datetime, timedelta

import pytest
from fastapi import FastAPI
from httpx import ASGITransport, AsyncClient

from app.core.security import hash_token_identifier
from app.database import get_db
from app.models.user import User, UserRole, UserStatus
from app.routers.auth_simple import router


class _Result:
    def __init__(self, value):
        self.value = value

    def scalar_one_or_none(self):
        return self.value


class _VerificationDb:
    def __init__(self, user: User):
        self.user = user
        self.execute_count = 0
        self.committed = False

    async def execute(self, _statement):
        self.execute_count += 1
        return _Result(self.user)

    async def commit(self):
        self.committed = True


@pytest.mark.asyncio
async def test_email_verification_is_a_post_mutation():
    token = "verification-token"
    user = User(
        email="candidate@example.test",
        password_hash="hash",
        role=UserRole.BUYER,
        status=UserStatus.PENDING,
        email_verified=False,
        email_verification_token_hash=hash_token_identifier(token),
        email_verification_token_expires_at=datetime.now(UTC) + timedelta(hours=1),
    )
    db = _VerificationDb(user)
    app = FastAPI()
    app.include_router(router, prefix="/api")

    async def override_db():
        yield db

    app.dependency_overrides[get_db] = override_db
    async with AsyncClient(
        transport=ASGITransport(app=app), base_url="https://test"
    ) as client:
        response = await client.post(f"/api/auth/verify-email?token={token}")
        repeated = await client.post(f"/api/auth/verify-email?token={token}")
        legacy_get = await client.get(f"/api/auth/verify-email?token={token}")

    assert response.status_code == 200
    assert repeated.status_code == 200
    assert legacy_get.status_code == 405
    assert user.email_verified is True
    assert user.email_verification_token_hash == hash_token_identifier(token)
    assert db.committed is True
