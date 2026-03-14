"""Unit tests for referral model and code generation."""
import pytest
from app.models.referral import Referral, ReferralStatus, generate_referral_code


class TestReferralCode:
    def test_generate_code_format(self):
        code = generate_referral_code()
        assert code.startswith("VDX-")
        assert len(code) == 10  # "VDX-" + 6 chars

    def test_generate_code_alphanumeric(self):
        code = generate_referral_code()
        suffix = code[4:]  # after "VDX-"
        assert suffix.isalnum()
        assert suffix == suffix.upper()

    def test_generate_code_unique(self):
        codes = {generate_referral_code() for _ in range(100)}
        assert len(codes) == 100  # all unique


class TestReferralStatus:
    def test_status_values(self):
        assert ReferralStatus.SIGNED_UP == "SIGNED_UP"
        assert ReferralStatus.VERIFIED == "VERIFIED"
        assert ReferralStatus.ACTIVE == "ACTIVE"


class TestReferralModel:
    def test_referral_has_required_columns(self):
        """Verify the model has all expected columns."""
        from sqlalchemy import inspect
        mapper = inspect(Referral)
        columns = {c.key for c in mapper.column_attrs}
        expected = {
            "id", "referrer_id", "referred_user_id",
            "referral_code_used", "status",
            "created_at", "verified_at", "activated_at",
        }
        assert expected.issubset(columns)
