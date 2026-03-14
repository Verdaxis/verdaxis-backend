"""Unit tests for referral Pydantic schemas."""
import pytest
from uuid import uuid4
from datetime import datetime, UTC
from pydantic import ValidationError

from app.schemas.referral import (
    ReferralCodeResponse,
    ReferralInviteRequest,
    ReferralListItem,
    LeaderboardEntry,
    ResolveCodeResponse,
)


class TestReferralInviteRequest:
    def test_valid_email(self):
        req = ReferralInviteRequest(email="test@example.com")
        assert req.email == "test@example.com"

    def test_invalid_email_rejected(self):
        with pytest.raises(ValidationError):
            ReferralInviteRequest(email="not-an-email")


class TestReferralCodeResponse:
    def test_has_code_and_link(self):
        resp = ReferralCodeResponse(referral_code="VDX-ABC123", referral_link="https://app.verdaxis.exchange/invite/VDX-ABC123")
        assert resp.referral_code == "VDX-ABC123"
        assert "invite" in resp.referral_link


class TestLeaderboardEntry:
    def test_fields(self):
        entry = LeaderboardEntry(
            rank=1,
            user_name="John Doe",
            organization_name="Acme Corp",
            referral_count=15,
        )
        assert entry.rank == 1
        assert entry.referral_count == 15
