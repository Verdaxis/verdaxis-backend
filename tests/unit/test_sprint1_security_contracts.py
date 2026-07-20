"""Sprint 1 identity and security contracts.

These tests intentionally describe the security boundary before its
implementation is added to the branch.
"""

from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import jwt
import pytest

from app.config import Settings, settings
from app.core.security import (
    create_access_token,
    create_refresh_token,
    create_stream_token,
    decode_token,
    get_password_hash,
    verify_password,
)
from app.middleware.execution import require_execution_eligible_user
from app.models.user import UserRole, UserStatus
from app.services.email_domains import (
    is_public_email_domain,
    registration_organization_domain,
)
from app.services.event_bus import event_bus


def _user(**overrides):
    values = {
        "id": uuid4(),
        "role": UserRole.BUYER,
        "status": UserStatus.APPROVED,
        "email_verified": True,
        "organization_id": uuid4(),
        "kyc_status": "APPROVED",
        "must_change_password": False,
        "kyc_external_evidence_reference": "external-test-case",
        "kyc_review_note": "Externally retained evidence reviewed.",
        "kyc_reviewed_by": uuid4(),
        "kyc_reviewed_at": datetime.now(UTC),
    }
    values.update(overrides)
    return SimpleNamespace(**values)


def _org(status="APPROVED", org_id=None):
    return SimpleNamespace(id=org_id, verification_status=status)


def _db_for_org(org):
    db = AsyncMock()
    result = MagicMock()
    result.scalar_one_or_none.return_value = org
    db.execute.return_value = result
    return db


def test_public_email_domains_are_not_tenant_boundaries(monkeypatch):
    from app.services import email_domains

    monkeypatch.setattr(email_domains, "DOMAIN_DATASETS_AVAILABLE", True)
    monkeypatch.setattr(email_domains, "PUBLIC_EMAIL_DOMAINS", frozenset({"gmail.com"}))
    monkeypatch.setattr(email_domains, "DISPOSABLE_EMAIL_DOMAINS", frozenset())
    assert is_public_email_domain(" Gmail.COM ") is True
    assert is_public_email_domain("company.example") is False
    assert registration_organization_domain("person@gmail.com") is None
    assert registration_organization_domain("person@company.example") == "company.example"


def test_bcrypt_handles_passwords_longer_than_72_bytes_without_truncation():
    first = "x" * 72 + "-one"
    second = "x" * 72 + "-two"
    hashed = get_password_hash(first)

    assert verify_password(first, hashed) is True
    assert verify_password(second, hashed) is False


def test_jwt_tokens_are_bound_to_issuer_and_audience(monkeypatch):
    monkeypatch.setattr(settings, "JWT_ISSUER", "test-api")
    monkeypatch.setattr(settings, "JWT_AUDIENCE", "test-web")
    token = create_access_token("user")
    payload = decode_token(token)

    assert payload["iss"] == "test-api"
    assert payload["aud"] == "test-web"

    monkeypatch.setattr(settings, "JWT_AUDIENCE", "other-web")
    with pytest.raises(jwt.InvalidAudienceError):
        decode_token(token)


def test_each_non_access_token_has_expected_type_and_claims():
    for token, token_type in (
        (create_refresh_token("user"), "refresh"),
        (create_stream_token("user", "organization"), "stream"),
        (
            create_access_token(
                "user",
                expires_delta=timedelta(minutes=30),
                additional_claims={"type": "registration"},
            ),
            "registration",
        ),
    ):
        payload = decode_token(token)
        assert payload["type"] == token_type
        assert payload["iss"] == settings.JWT_ISSUER
        assert payload["aud"] == settings.JWT_AUDIENCE


@pytest.mark.asyncio
async def test_execution_dependency_rejects_admin_market_execution():
    user = _user(
        role=UserRole.ADMIN,
        email_verified=False,
        organization_id=None,
        kyc_status="PENDING",
    )

    with pytest.raises(Exception) as exc_info:
        await require_execution_eligible_user(user, AsyncMock())
    assert exc_info.value.status_code == 403


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "changes",
    [
        {"email_verified": False},
        {"kyc_status": "REJECTED"},
        {"organization_id": None},
    ],
)
async def test_execution_dependency_rejects_nonqualified_market_users(changes):
    user = _user(**changes)
    db = _db_for_org(_org(org_id=user.organization_id))

    with pytest.raises(Exception) as exc_info:
        await require_execution_eligible_user(user, db)

    assert getattr(exc_info.value, "status_code", None) == 403


@pytest.mark.asyncio
async def test_execution_dependency_requires_approved_organization():
    user = _user()

    with pytest.raises(Exception) as exc_info:
        await require_execution_eligible_user(
            user, _db_for_org(_org("PENDING", org_id=user.organization_id))
        )

    assert exc_info.value.status_code == 403


def test_stream_trade_events_are_published_only_to_participant_channels():
    from app.services.activity import publish_trade_event

    trade = SimpleNamespace(buyer_id=uuid4(), seller_id=uuid4())
    captured = []

    async def capture(channel, event_type, data):
        captured.append((channel, event_type, data))

    with patch.object(event_bus, "publish", side_effect=capture):
        import asyncio

        asyncio.run(publish_trade_event(trade, "trade_confirmed", {"id": "trade"}))

    assert {row[0] for row in captured} == {
        f"trades:{trade.buyer_id}",
        f"trades:{trade.seller_id}",
    }


def test_sqladmin_is_disabled_by_default_and_requires_distinct_secret():
    assert Settings.model_fields["ENABLE_SQLADMIN"].default is False
    with pytest.raises(ValueError):
        Settings(
            ENVIRONMENT="test",
            JWT_SECRET="test-secret-key-for-testing-minimum-32-chars",
            ENABLE_SQLADMIN=True,
            ADMIN_SESSION_SECRET=None,
        )
