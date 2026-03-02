"""
Unit tests for OAuth SSO integration.

Tests the OAuth router logic — provider login redirects, callback handling,
user creation, domain matching, and token issuance — using mocked Authlib
responses (no real OAuth providers hit).
"""
import uuid
from datetime import datetime, UTC
from unittest.mock import AsyncMock, MagicMock, patch

import pytest
from starlette.testclient import TestClient

from app.core.security import decode_token


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

@pytest.fixture
def app_client():
    """Create a test client with the full FastAPI app (sync, for redirect tests)."""
    from app.main import app
    # TestClient follows redirects by default — disable to inspect redirect URLs
    return TestClient(app, follow_redirects=False)


def _make_mock_user(
    email="test@example.com",
    first_name="Test",
    last_name="User",
    status="APPROVED",
    role="BUYER",
    oauth_provider=None,
    org_id=None,
):
    """Build a mock User object that looks like the ORM model."""
    user = MagicMock()
    user.id = uuid.uuid4()
    user.email = email
    user.first_name = first_name
    user.last_name = last_name
    user.status = MagicMock()
    user.status.value = status
    user.status.__eq__ = lambda self, other: self.value == (other.value if hasattr(other, "value") else other)
    user.status.__ne__ = lambda self, other: not self.__eq__(other)
    user.role = MagicMock()
    user.role.value = role
    user.oauth_provider = oauth_provider
    user.organization_id = org_id
    user.last_login = None
    user.password_hash = None
    return user


# ---------------------------------------------------------------------------
# Google Login Redirect
# ---------------------------------------------------------------------------

class TestGoogleLoginRedirect:
    def test_google_login_redirects_to_google(self, app_client):
        """GET /oauth/google/login should redirect to Google's auth endpoint."""
        with patch("app.routers.oauth.settings") as mock_settings:
            mock_settings.GOOGLE_CLIENT_ID = "test-google-id"
            mock_settings.GOOGLE_CLIENT_SECRET = "test-google-secret"
            mock_settings.OAUTH_REDIRECT_BASE = "https://api.verdaxis.exchange"

            # The authorize_redirect will try to build a real redirect to Google.
            # We mock it to return a controlled redirect.
            with patch("app.routers.oauth.oauth") as mock_oauth:
                from starlette.responses import RedirectResponse
                mock_oauth.google.authorize_redirect = AsyncMock(
                    return_value=RedirectResponse(
                        url="https://accounts.google.com/o/oauth2/v2/auth?client_id=test"
                    )
                )
                resp = app_client.get("/oauth/google/login")
                assert resp.status_code == 307
                assert "accounts.google.com" in resp.headers["location"]

    def test_google_login_returns_501_when_not_configured(self, app_client):
        """GET /oauth/google/login returns 501 if GOOGLE_CLIENT_ID is empty."""
        with patch("app.routers.oauth.settings") as mock_settings:
            mock_settings.GOOGLE_CLIENT_ID = ""
            resp = app_client.get("/oauth/google/login")
            assert resp.status_code == 501
            assert "not configured" in resp.json()["detail"]


# ---------------------------------------------------------------------------
# Microsoft Login Redirect
# ---------------------------------------------------------------------------

class TestMicrosoftLoginRedirect:
    def test_microsoft_login_redirects(self, app_client):
        """GET /oauth/microsoft/login should redirect to Microsoft's auth endpoint."""
        with patch("app.routers.oauth.settings") as mock_settings:
            mock_settings.MICROSOFT_CLIENT_ID = "test-ms-id"
            mock_settings.MICROSOFT_CLIENT_SECRET = "test-ms-secret"
            mock_settings.OAUTH_REDIRECT_BASE = "https://api.verdaxis.exchange"

            with patch("app.routers.oauth.oauth") as mock_oauth:
                from starlette.responses import RedirectResponse
                mock_oauth.microsoft.authorize_redirect = AsyncMock(
                    return_value=RedirectResponse(
                        url="https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=test"
                    )
                )
                resp = app_client.get("/oauth/microsoft/login")
                assert resp.status_code == 307
                assert "login.microsoftonline.com" in resp.headers["location"]

    def test_microsoft_login_returns_501_when_not_configured(self, app_client):
        """GET /oauth/microsoft/login returns 501 if MICROSOFT_CLIENT_ID is empty."""
        with patch("app.routers.oauth.settings") as mock_settings:
            mock_settings.MICROSOFT_CLIENT_ID = ""
            resp = app_client.get("/oauth/microsoft/login")
            assert resp.status_code == 501
            assert "not configured" in resp.json()["detail"]


# ---------------------------------------------------------------------------
# Google Callback — existing approved user
# ---------------------------------------------------------------------------

class TestGoogleCallbackExistingUser:
    def test_existing_approved_user_gets_tokens(self, app_client):
        """Callback with known approved user redirects to frontend with tokens."""
        mock_user = _make_mock_user(
            email="alice@acme.com",
            status="APPROVED",
            oauth_provider="google",
        )
        # Make status comparison work with UserStatus enum
        from app.models.user import UserStatus
        mock_user.status = UserStatus.APPROVED

        mock_token = {
            "userinfo": {
                "email": "alice@acme.com",
                "given_name": "Alice",
                "family_name": "Smith",
            }
        }

        with patch("app.routers.oauth.oauth") as mock_oauth, \
             patch("app.routers.oauth._find_or_create_user", new_callable=AsyncMock) as mock_find:
            mock_oauth.google.authorize_access_token = AsyncMock(return_value=mock_token)
            mock_find.return_value = mock_user

            resp = app_client.get("/oauth/google/callback?code=mock-auth-code&state=mock")
            assert resp.status_code == 307
            location = resp.headers["location"]
            assert "app.verdaxis.exchange/app" in location
            assert "token=" in location
            assert "refresh=" in location

    def test_callback_oauth_error_redirects_to_login(self, app_client):
        """OAuthError during token exchange redirects to login with error."""
        from authlib.integrations.starlette_client import OAuthError
        with patch("app.routers.oauth.oauth") as mock_oauth:
            mock_oauth.google.authorize_access_token = AsyncMock(
                side_effect=OAuthError(error="access_denied", description="user denied")
            )
            resp = app_client.get("/oauth/google/callback?code=bad&state=x")
            assert resp.status_code == 307
            assert "error=oauth_failed" in resp.headers["location"]


# ---------------------------------------------------------------------------
# Callback — pending user (not yet approved)
# ---------------------------------------------------------------------------

class TestCallbackPendingUser:
    def test_pending_user_redirected_with_error(self, app_client):
        """New/pending user is redirected to login with account_pending error."""
        from app.models.user import UserStatus
        mock_user = _make_mock_user(email="new@startup.com", status="PENDING")
        mock_user.status = UserStatus.PENDING

        mock_token = {
            "userinfo": {
                "email": "new@startup.com",
                "given_name": "New",
                "family_name": "User",
            }
        }

        with patch("app.routers.oauth.oauth") as mock_oauth, \
             patch("app.routers.oauth._find_or_create_user", new_callable=AsyncMock) as mock_find:
            mock_oauth.google.authorize_access_token = AsyncMock(return_value=mock_token)
            mock_find.return_value = mock_user

            resp = app_client.get("/oauth/google/callback?code=mock&state=s")
            assert resp.status_code == 307
            assert "error=account_pending" in resp.headers["location"]


# ---------------------------------------------------------------------------
# _find_or_create_user logic (unit-tested directly)
# ---------------------------------------------------------------------------

class TestFindOrCreateUser:
    @pytest.mark.asyncio
    async def test_creates_new_user_when_not_found(self):
        """Auto-creates a PENDING user when email is unknown."""
        from app.routers.oauth import _find_or_create_user

        mock_db = AsyncMock()
        # First query: user lookup — not found
        mock_user_result = MagicMock()
        mock_user_result.scalar_one_or_none.return_value = None
        # Second query: org lookup — not found
        mock_org_result = MagicMock()
        mock_org_result.scalar_one_or_none.return_value = None

        mock_db.execute = AsyncMock(side_effect=[mock_user_result, mock_org_result])
        mock_db.add = MagicMock()
        mock_db.commit = AsyncMock()
        mock_db.refresh = AsyncMock()

        user = await _find_or_create_user(
            db=mock_db,
            email="new@unknown.com",
            first_name="New",
            last_name="User",
            provider="google",
        )

        # Should have called db.add with a new User
        mock_db.add.assert_called_once()
        added_user = mock_db.add.call_args[0][0]
        assert added_user.email == "new@unknown.com"
        assert added_user.oauth_provider == "google"
        assert added_user.password_hash is None
        assert added_user.organization_id is None

    @pytest.mark.asyncio
    async def test_links_to_org_by_domain(self):
        """Auto-links new user to organization when email domain matches."""
        from app.routers.oauth import _find_or_create_user

        mock_org = MagicMock()
        mock_org.id = uuid.uuid4()

        mock_db = AsyncMock()
        mock_user_result = MagicMock()
        mock_user_result.scalar_one_or_none.return_value = None
        mock_org_result = MagicMock()
        mock_org_result.scalar_one_or_none.return_value = mock_org

        mock_db.execute = AsyncMock(side_effect=[mock_user_result, mock_org_result])
        mock_db.add = MagicMock()
        mock_db.commit = AsyncMock()
        mock_db.refresh = AsyncMock()

        user = await _find_or_create_user(
            db=mock_db,
            email="alice@acme.com",
            first_name="Alice",
            last_name="Acme",
            provider="google",
        )

        added_user = mock_db.add.call_args[0][0]
        assert added_user.organization_id == mock_org.id

    @pytest.mark.asyncio
    async def test_existing_user_not_duplicated(self):
        """Existing user is returned without creating a duplicate."""
        from app.routers.oauth import _find_or_create_user

        existing = MagicMock()
        existing.id = uuid.uuid4()
        existing.email = "existing@acme.com"
        existing.oauth_provider = None
        existing.last_login = None

        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = existing
        mock_db.execute = AsyncMock(return_value=mock_result)
        mock_db.commit = AsyncMock()
        mock_db.refresh = AsyncMock()

        user = await _find_or_create_user(
            db=mock_db,
            email="existing@acme.com",
            first_name="Existing",
            last_name="User",
            provider="google",
        )

        # Should NOT call db.add — user already exists
        mock_db.add.assert_not_called()
        # Should update oauth_provider
        assert existing.oauth_provider == "google"
        assert existing.last_login is not None

    @pytest.mark.asyncio
    async def test_existing_user_preserves_existing_provider(self):
        """If user already has an oauth_provider, don't overwrite it."""
        from app.routers.oauth import _find_or_create_user

        existing = MagicMock()
        existing.id = uuid.uuid4()
        existing.email = "existing@acme.com"
        existing.oauth_provider = "microsoft"
        existing.last_login = None

        mock_db = AsyncMock()
        mock_result = MagicMock()
        mock_result.scalar_one_or_none.return_value = existing
        mock_db.execute = AsyncMock(return_value=mock_result)
        mock_db.commit = AsyncMock()
        mock_db.refresh = AsyncMock()

        user = await _find_or_create_user(
            db=mock_db,
            email="existing@acme.com",
            first_name="Existing",
            last_name="User",
            provider="google",
        )

        # Should NOT overwrite existing provider
        assert existing.oauth_provider == "microsoft"


# ---------------------------------------------------------------------------
# Token issuance validation
# ---------------------------------------------------------------------------

class TestTokenIssuance:
    def test_build_redirect_contains_valid_tokens(self):
        """_build_redirect returns a URL with decodable access and refresh tokens."""
        from app.routers.oauth import _build_redirect
        from app.models.user import UserRole, UserStatus

        mock_user = MagicMock()
        mock_user.id = uuid.uuid4()
        mock_user.role = UserRole.BUYER
        mock_user.status = UserStatus.APPROVED

        response = _build_redirect(mock_user)
        location = response.headers["location"]

        # Extract tokens from the redirect URL
        from urllib.parse import urlparse, parse_qs
        parsed = urlparse(location)
        params = parse_qs(parsed.query)

        assert "token" in params
        assert "refresh" in params

        # Verify the access token is decodable
        access_payload = decode_token(params["token"][0])
        assert access_payload["sub"] == str(mock_user.id)
        assert access_payload["type"] == "access"
        assert access_payload["role"] == "BUYER"

        # Verify the refresh token is decodable
        refresh_payload = decode_token(params["refresh"][0])
        assert refresh_payload["sub"] == str(mock_user.id)
        assert refresh_payload["type"] == "refresh"


# ---------------------------------------------------------------------------
# Callback with missing userinfo fields
# ---------------------------------------------------------------------------

class TestCallbackEdgeCases:
    def test_callback_no_email_redirects_with_error(self, app_client):
        """If Google returns userinfo without email, redirect to login with error."""
        mock_token = {
            "userinfo": {
                "given_name": "NoEmail",
                # no "email" field
            }
        }
        with patch("app.routers.oauth.oauth") as mock_oauth:
            mock_oauth.google.authorize_access_token = AsyncMock(return_value=mock_token)
            resp = app_client.get("/oauth/google/callback?code=c&state=s")
            assert resp.status_code == 307
            assert "error=oauth_no_email" in resp.headers["location"]

    def test_callback_no_userinfo_redirects_with_error(self, app_client):
        """If token response has no userinfo, redirect to login with error."""
        mock_token = {"access_token": "xxx"}  # no userinfo key
        with patch("app.routers.oauth.oauth") as mock_oauth:
            mock_oauth.google.authorize_access_token = AsyncMock(return_value=mock_token)
            resp = app_client.get("/oauth/google/callback?code=c&state=s")
            assert resp.status_code == 307
            assert "error=oauth_no_profile" in resp.headers["location"]
