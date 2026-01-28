"""
Unit tests for security/authentication utilities.
"""
import pytest
from app.core.security import verify_password, get_password_hash


class TestPasswordHashing:
    """Tests for password hashing and verification."""
    
    def test_get_password_hash_returns_hash(self):
        """Should return a hash different from the original password."""
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        
        assert hashed != password
        assert len(hashed) > 20  # Hashes are longer than typical passwords
    
    def test_get_password_hash_different_each_time(self):
        """Should return different hashes for the same password (due to salting)."""
        password = "mysecurepassword"
        hash1 = get_password_hash(password)
        hash2 = get_password_hash(password)
        
        # Hashes should be different due to unique salts
        assert hash1 != hash2
    
    def test_verify_password_correct(self):
        """Should return True for correct password."""
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        
        assert verify_password(password, hashed) is True
    
    def test_verify_password_incorrect(self):
        """Should return False for incorrect password."""
        password = "mysecurepassword"
        wrong_password = "wrongpassword"
        hashed = get_password_hash(password)
        
        assert verify_password(wrong_password, hashed) is False
    
    def test_verify_password_empty_string(self):
        """Should handle empty password comparison."""
        password = "mysecurepassword"
        hashed = get_password_hash(password)
        
        assert verify_password("", hashed) is False
    
    def test_hash_empty_password(self):
        """Should be able to hash empty password (though not recommended)."""
        hashed = get_password_hash("")
        assert hashed is not None
        assert len(hashed) > 0


class TestTokenCreation:
    """Tests for JWT token creation."""
    
    def test_create_access_token_returns_string(self):
        """Should return a JWT token string."""
        from app.core.auth import create_access_token
        
        token = create_access_token(data={"sub": "user123", "role": "BUYER"})
        
        assert isinstance(token, str)
        assert len(token) > 0
        # JWT format: header.payload.signature
        assert token.count('.') == 2
    
    def test_create_access_token_with_custom_expiry(self):
        """Should accept custom expiry delta."""
        from app.core.auth import create_access_token
        from datetime import timedelta
        
        token = create_access_token(
            data={"sub": "user123"},
            expires_delta=timedelta(minutes=30)
        )
        
        assert isinstance(token, str)
        assert token.count('.') == 2
    
    def test_token_contains_user_data(self):
        """Token should contain the provided user data."""
        from app.core.auth import create_access_token
        from jose import jwt
        from app.config import settings
        
        user_id = "test-user-id-123"
        role = "SUPPLIER"
        
        token = create_access_token(data={"sub": user_id, "role": role})
        
        # Decode without verification to check payload
        payload = jwt.decode(token, settings.JWT_SECRET, algorithms=["HS256"])
        
        assert payload["sub"] == user_id
        assert payload["role"] == role
        assert "exp" in payload
