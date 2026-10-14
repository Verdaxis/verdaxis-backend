"""Indicative work preserves admission without adopting the execution KYC gate."""
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock
from uuid import uuid4

import pytest
from fastapi import HTTPException

from app.models.user import UserRole, UserStatus
from app.services.security_market_admission import require_security_market_admission


@pytest.mark.asyncio
@pytest.mark.parametrize("role,status,allowed", [
    (UserRole.BUYER, "PENDING", True),
    (UserRole.SUPPLIER, "SUBMITTED", True),
    (UserRole.ADMIN, "APPROVED", False),
    (UserRole.BUYER, "REJECTED", False),
])
async def test_indicative_admission_preserves_existing_rules(role, status, allowed):
    organization_id = uuid4()
    user = SimpleNamespace(id=uuid4(), organization_id=organization_id, role=role,
                           status=UserStatus.APPROVED, email_verified=True,
                           must_change_password=False, kyc_status=status, kyc_organization_id=None)
    organization = SimpleNamespace(id=organization_id, verification_status="APPROVED")
    db = SimpleNamespace(execute=AsyncMock(return_value=Mock(scalar_one_or_none=lambda: organization)))
    if allowed:
        assert await require_security_market_admission(user, db) is user
    else:
        with pytest.raises(HTTPException) as error:
            await require_security_market_admission(user, db)
        assert error.value.status_code == 403
