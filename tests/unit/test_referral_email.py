"""Unit tests for referral invite email."""
import pytest
from unittest.mock import patch, AsyncMock


class TestSendReferralInviteEmail:
    @pytest.mark.asyncio
    async def test_sends_email(self):
        with patch("app.services.email._send_email", new_callable=AsyncMock, return_value=True) as mock_send:
            from app.services.email import send_referral_invite_email
            result = await send_referral_invite_email(
                to_email="new@example.com",
                referrer_name="John Doe",
                referral_code="VDX-ABC123",
            )
            assert result is True
            mock_send.assert_called_once()
            call_args = mock_send.call_args
            assert call_args[1]["to_email"] == "new@example.com"
            assert "John Doe" in call_args[1]["subject"]
            assert "VDX-ABC123" in call_args[1]["html"]

    @pytest.mark.asyncio
    async def test_invite_link_in_html(self):
        with patch("app.services.email._send_email", new_callable=AsyncMock, return_value=True) as mock_send:
            from app.services.email import send_referral_invite_email
            await send_referral_invite_email(
                to_email="new@example.com",
                referrer_name="Jane",
                referral_code="VDX-XYZ789",
            )
            html = mock_send.call_args[1]["html"]
            assert "/invite/VDX-XYZ789" in html
