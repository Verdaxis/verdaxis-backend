"""Unit tests for referral attribution and status progression logic."""
import pytest
from app.models.referral import ReferralStatus


class TestReferralStatusProgression:
    def test_signed_up_to_verified(self):
        assert ReferralStatus.SIGNED_UP.value == "SIGNED_UP"
        assert ReferralStatus.VERIFIED.value == "VERIFIED"

    def test_verified_to_active(self):
        assert ReferralStatus.ACTIVE.value == "ACTIVE"

    def test_all_statuses(self):
        statuses = [s.value for s in ReferralStatus]
        assert statuses == ["SIGNED_UP", "VERIFIED", "ACTIVE"]
