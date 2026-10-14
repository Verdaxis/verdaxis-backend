from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, Mock
from uuid import uuid4

import pytest
from fastapi import HTTPException

from app.models.user import UserRole
from app.services.kyc_execution import kyc_execution_eligibility, require_kyc_execution


@pytest.mark.asyncio
@pytest.mark.parametrize("status,bound,remaining,reason", [
    ("APPROVED", True, None, "KYC_APPROVED"),
    ("APPROVED", False, None, "KYC_REQUIRED"),
    ("PENDING", False, 1, "KYC_GRACE"),
    ("SUBMITTED", True, 1, "KYC_GRACE"),
    ("PENDING", False, 0, "KYC_REQUIRED"),
    ("SUBMITTED", True, -1, "KYC_REQUIRED"),
    ("REJECTED", True, 1, "KYC_REJECTED"),
    ("REVIEW_REQUIRED", True, None, "KYC_REQUIRED"),
])
async def test_kyc_rule(status, bound, remaining, reason):
    now = datetime.now(UTC)
    org_id = uuid4()
    user = SimpleNamespace(id=uuid4(), organization_id=org_id, kyc_status=status,
                           kyc_organization_id=org_id if bound else None)
    grace = None if remaining is None else SimpleNamespace(expires_at=now + timedelta(seconds=remaining))
    db = SimpleNamespace(execute=AsyncMock(return_value=Mock(scalar_one_or_none=lambda: grace)))
    result = await kyc_execution_eligibility(db, user, now=now)
    assert result.reason == reason
    assert result.eligible == (reason in {"KYC_APPROVED", "KYC_GRACE"})


@pytest.mark.asyncio
async def test_blocked_error_has_stable_contract():
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4(), kyc_status="PENDING")
    db = SimpleNamespace(execute=AsyncMock(return_value=Mock(scalar_one_or_none=lambda: None)))
    with pytest.raises(HTTPException) as exc:
        await require_kyc_execution(db, user)
    assert exc.value.status_code == 403
    assert exc.value.detail == {"code": "KYC_REQUIRED", "message": "Complete KYC verification to trade.",
                                "kyc_status": "PENDING", "grace_expires_at": None}


def test_migration_grace_uses_elapsed_hours_not_calendar_days():
    from pathlib import Path

    migration = Path("alembic/versions/kycg_20261014_staging.py").read_text()
    assert "interval '336 hours'" in migration
    assert "interval '14 days'" not in migration


def test_customer_member_lookup_index_matches_migration():
    from pathlib import Path
    from app.models.user import User

    index = next(index for index in User.__table__.indexes if index.name == "ix_users_organization_id_id")
    assert [column.name for column in index.columns] == ["organization_id", "id"]
    migration = Path("alembic/versions/kycg_20261014_staging.py").read_text()
    assert 'op.create_index("ix_users_organization_id_id", "users", ["organization_id", "id"])' in migration
    assert 'op.drop_index("ix_users_organization_id_id", table_name="users")' in migration
