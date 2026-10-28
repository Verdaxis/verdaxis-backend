from types import SimpleNamespace
from uuid import uuid4

import pytest

from app.models.user import UserRole, UserStatus
from app.services.execution_policy import execution_party_is_eligible
from app.routers.kyc import _require_current_kyc_organization
from fastapi import HTTPException


def _user(org_id, *, kyc_status="PENDING", kyc_org_id=None):
    return SimpleNamespace(
        id=uuid4(), organization_id=org_id, role=UserRole.BUYER,
        status=UserStatus.APPROVED, email_verified=True, must_change_password=False,
        kyc_status=kyc_status, kyc_organization_id=kyc_org_id,
    )


def _org(org_id):
    return SimpleNamespace(id=org_id, verification_status="APPROVED")


@pytest.mark.asyncio
@pytest.mark.parametrize("kyc_status", ["PENDING", "SUBMITTED", "REVIEW_REQUIRED"])
async def test_unapproved_kyc_without_grace_blocks_execution(kyc_status):
    from unittest.mock import AsyncMock, Mock

    org_id = uuid4()
    db = SimpleNamespace(execute=AsyncMock(return_value=Mock(scalar_one_or_none=lambda: None)))
    assert not await execution_party_is_eligible(
        db, user=_user(org_id, kyc_status=kyc_status), organization=_org(org_id)
    )


@pytest.mark.asyncio
async def test_bound_kyc_must_match_current_org_and_rejection_revokes_execution():
    org_id = uuid4()
    assert not await execution_party_is_eligible(
        SimpleNamespace(),
        user=_user(org_id, kyc_status="APPROVED", kyc_org_id=uuid4()),
        organization=_org(org_id),
    )
    assert not await execution_party_is_eligible(
        SimpleNamespace(),
        user=_user(org_id, kyc_status="REJECTED", kyc_org_id=org_id),
        organization=_org(org_id),
    )


def test_kyc_model_has_explicit_organization_provenance():
    from app.models.user import User

    assert hasattr(User, "kyc_organization_id")


def test_admin_review_rejects_evidence_from_a_previous_org():
    target = _user(uuid4(), kyc_status="SUBMITTED", kyc_org_id=uuid4())
    with pytest.raises(HTTPException) as exc_info:
        _require_current_kyc_organization(target)
    assert exc_info.value.status_code == 409
    assert exc_info.value.detail["code"] == "KYC_ORGANIZATION_MISMATCH"
