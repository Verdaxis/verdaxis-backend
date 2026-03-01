"""
Unit tests for security/authentication utilities.
Updated for PyJWT + direct bcrypt (2026-03-01).
"""
import pytest
from app.core.security import (
    verify_password, get_password_hash,
    create_access_token, create_refresh_token, decode_token,
)


class TestPasswordHashing:
    def test_get_password_hash_returns_hash(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert hashed != password
        assert len(hashed) > 20

    def test_get_password_hash_different_each_time(self):
        password = "mysecurepassword"
        hash1 = get_password_hash(password)
        hash2 = get_password_hash(password)
        assert hash1 != hash2

    def test_verify_password_correct(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password(password, hashed) is True

    def test_verify_password_incorrect(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password("wrongpassword", hashed) is False

    def test_verify_password_empty_string(self):
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        assert verify_password("", hashed) is False

    def test_hash_empty_password(self):
        hashed = get_password_hash("")
        assert hashed is not None
        assert len(hashed) > 0


class TestTokenCreation:
    def test_create_access_token_returns_jwt(self):
        token = create_access_token(subject="user123")
        assert isinstance(token, str)
        assert token.count('.') == 2

    def test_create_access_token_with_custom_expiry(self):
        from datetime import timedelta
        token = create_access_token(subject="user123", expires_delta=timedelta(minutes=30))
        assert isinstance(token, str)
        assert token.count('.') == 2

    def test_token_contains_user_data(self):
        user_id = "test-user-id-123"
        token = create_access_token(
            subject=user_id,
            additional_claims={"role": "SUPPLIER"},
        )
        payload = decode_token(token)
        assert payload["sub"] == user_id
        assert payload["role"] == "SUPPLIER"
        assert payload["type"] == "access"
        assert "exp" in payload
        assert "iat" in payload

    def test_refresh_token_has_correct_type(self):
        token = create_refresh_token(subject="user123")
        payload = decode_token(token)
        assert payload["type"] == "refresh"
        assert "exp" in payload
        assert "iat" in payload

    def test_access_token_rejected_as_refresh(self):
        """Access tokens should have type='access', not 'refresh'."""
        token = create_access_token(subject="user123")
        payload = decode_token(token)
        assert payload["type"] == "access"

    def test_expired_token_raises(self):
        from datetime import timedelta
        import jwt as pyjwt
        token = create_access_token(subject="user123", expires_delta=timedelta(seconds=-1))
        with pytest.raises(pyjwt.ExpiredSignatureError):
            decode_token(token)
