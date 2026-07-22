from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from uuid import uuid4

import pytest

from app.config import Settings, settings
from app.core.security import create_refresh_token, decode_token
from app.models.user import UserRole, UserStatus
from app.schemas.user import UserCreate
from app.routers.auth_simple import (
    MAX_PASSWORD_BYTES,
    _new_registration_token,
    validate_password_bytes,
)
from app.services.execution_policy import execution_party_is_eligible
from app.services.kyc import KYC_REVIEW_REQUIRED


def test_production_jwt_boundaries_are_explicit_and_not_defaulted():
    with pytest.raises(ValueError):
        Settings(
            ENVIRONMENT="production",
            JWT_SECRET="x" * 64,
            JWT_ISSUER=None,
            JWT_AUDIENCE=None,
        )


def test_refresh_token_requires_durable_rotation_claims():
    payload = decode_token(create_refresh_token(str(uuid4())))
    assert payload["jti"]
    assert payload["family_id"]


def test_registration_token_is_opaque_and_contains_no_password_hash():
    token = _new_registration_token()
    assert token.raw != token.token_hash
    assert "password_hash" not in token.raw
    assert len(token.raw) >= 32


def test_password_limits_are_byte_based():
    with pytest.raises(ValueError):
        validate_password_bytes("x" * (MAX_PASSWORD_BYTES + 1))
    validate_password_bytes("pässword-123")


def test_email_identity_is_normalized_at_schema_boundary():
    user = UserCreate(email="  Person@Example.COM ", password="password-123", role=UserRole.BUYER)
    assert user.email == "person@example.com"


def test_kyc_provider_result_is_review_required_not_approval():
    assert KYC_REVIEW_REQUIRED == "SUBMITTED"


@pytest.mark.asyncio
async def test_execution_party_requires_user_owner_and_approved_org():
    user = SimpleNamespace(
        id=uuid4(),
        organization_id=uuid4(),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        kyc_status="APPROVED",
        must_change_password=False,
        kyc_external_evidence_reference="external-test-case",
        kyc_review_note="Externally retained evidence reviewed.",
        kyc_reviewed_by=uuid4(),
        kyc_reviewed_at=datetime.now(UTC),
    )
    org = SimpleNamespace(id=user.organization_id, verification_status="APPROVED")
    db = SimpleNamespace()
    assert await execution_party_is_eligible(db, user=user, organization=org) is True
