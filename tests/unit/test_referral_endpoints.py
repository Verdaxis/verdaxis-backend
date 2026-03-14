"""Unit tests for referral router logic — no DB required."""
import pytest
import uuid
from unittest.mock import AsyncMock, MagicMock, patch
from datetime import datetime, UTC

from app.models.referral import generate_referral_code


class TestGenerateReferralCode:
    def test_format(self):
        code = generate_referral_code()
        assert code.startswith("VDX-")
        assert len(code) == 10

    def test_uniqueness(self):
        codes = {generate_referral_code() for _ in range(200)}
        assert len(codes) == 200


class TestResolveCodeLogic:
    """Tests for the public resolve endpoint logic."""

    def test_invalid_code_format(self):
        """Codes must be VDX- prefix + 6 alphanumeric."""
        from app.routers.referrals import _validate_code_format
        assert _validate_code_format("VDX-ABC123") is True
        assert _validate_code_format("VDX-abc123") is False  # lowercase
        assert _validate_code_format("ABC123") is False  # no prefix
        assert _validate_code_format("VDX-AB") is False  # too short
        assert _validate_code_format("") is False
