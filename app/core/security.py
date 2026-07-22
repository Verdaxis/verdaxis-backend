"""
Security utilities: password hashing (bcrypt) and JWT token management (PyJWT).

Migrated from python-jose + passlib to PyJWT + direct bcrypt (2026-03-01).
- python-jose: unmaintained since 2022, known CVEs
- passlib: unmaintained since 2020, breaks on Python 3.13
"""
from datetime import datetime, timedelta, UTC
from typing import Any, Union
import hashlib
import secrets
import uuid

import bcrypt
import jwt

from app.config import settings

# JWT configuration is read from validated Settings at issuance/decoding time
# so staged key rotation does not leave a stale module-level secret behind.
ACCESS_TOKEN_EXPIRE_MINUTES = settings.ACCESS_TOKEN_EXPIRE_MINUTES  # 15 min
REFRESH_TOKEN_EXPIRE_DAYS = settings.REFRESH_TOKEN_EXPIRE_DAYS      # 7 days
MAX_PASSWORD_BYTES = 1024


# ---------------------------------------------------------------------------
# Password Hashing (direct bcrypt — no passlib wrapper)
# ---------------------------------------------------------------------------

def verify_password(plain_password: str, hashed_password: str) -> bool:
    """Verifies a plain password against a bcrypt hash."""
    return bcrypt.checkpw(
        _bcrypt_input(plain_password),
        hashed_password.encode("utf-8"),
    )


def get_password_hash(password: str) -> str:
    """Hashes a password using bcrypt with automatic salt."""
    return bcrypt.hashpw(
        _bcrypt_input(password),
        bcrypt.gensalt(),
    ).decode("utf-8")


def _bcrypt_input(password: str) -> bytes:
    """Keep bcrypt's 72-byte limit explicit without silent truncation."""
    encoded = password.encode("utf-8")
    validate_password_bytes(password)
    return hashlib.sha256(encoded).digest() if len(encoded) > 72 else encoded


def validate_password_bytes(password: str) -> str:
    if len(password.encode("utf-8")) > MAX_PASSWORD_BYTES:
        raise ValueError(f"Password must be no more than {MAX_PASSWORD_BYTES} UTF-8 bytes")
    return password


# ---------------------------------------------------------------------------
# JWT Token Creation
# ---------------------------------------------------------------------------

def create_access_token(
    subject: Union[str, Any],
    expires_delta: timedelta | None = None,
    additional_claims: dict | None = None,
) -> str:
    """Creates a short-lived JWT access token (default 15 min)."""
    now = datetime.now(UTC)
    expire = now + (expires_delta or timedelta(minutes=ACCESS_TOKEN_EXPIRE_MINUTES))
    to_encode: dict[str, Any] = {
        "sub": str(subject),
        "exp": expire,
        "type": "access",
        "iat": now,
        "iat_us": int(now.timestamp() * 1_000_000),
        "iss": settings.JWT_ISSUER,
        "aud": settings.JWT_AUDIENCE,
    }
    if additional_claims:
        to_encode.update(additional_claims)
    return jwt.encode(to_encode, settings.JWT_SECRET, algorithm=settings.JWT_ALGORITHM)


def create_refresh_token(subject: Union[str, Any], *, family_id: str | None = None) -> str:
    """Creates a long-lived JWT refresh token (default 7 days)."""
    now = datetime.now(UTC)
    expire = now + timedelta(days=REFRESH_TOKEN_EXPIRE_DAYS)
    to_encode: dict[str, Any] = {
        "sub": str(subject),
        "exp": expire,
        "type": "refresh",
        "iat": now,
        "iat_us": int(now.timestamp() * 1_000_000),
        "jti": secrets.token_urlsafe(32),
        "family_id": family_id or str(uuid.uuid4()),
        "iss": settings.JWT_ISSUER,
        "aud": settings.JWT_AUDIENCE,
    }
    return jwt.encode(to_encode, settings.JWT_SECRET, algorithm=settings.JWT_ALGORITHM)


def create_stream_token(user_id: Union[str, Any], organization_id: Union[str, Any]) -> str:
    """Creates a single-purpose JWT for SSE query-param authentication."""
    now = datetime.now(UTC)
    expire = now + timedelta(seconds=60)
    to_encode: dict[str, Any] = {
        "sub": str(user_id),
        "org_id": str(organization_id),
        "environment": settings.ENVIRONMENT.strip().lower(),
        "exp": expire,
        "type": "stream",
        "iat": now,
        "iat_us": int(now.timestamp() * 1_000_000),
        "iss": settings.JWT_ISSUER,
        "aud": settings.JWT_AUDIENCE,
    }
    return jwt.encode(to_encode, settings.JWT_SECRET, algorithm=settings.JWT_ALGORITHM)


def decode_token(token: str) -> dict[str, Any]:
    """Decodes and validates a JWT token. Raises jwt.PyJWTError on failure."""
    secrets_to_try = [settings.JWT_SECRET]
    if settings.JWT_SECRET_PREVIOUS:
        secrets_to_try.append(settings.JWT_SECRET_PREVIOUS)
    last_error = None
    for secret in secrets_to_try:
        try:
            return jwt.decode(
                token,
                secret,
                algorithms=[settings.JWT_ALGORITHM],
                issuer=settings.JWT_ISSUER,
                audience=settings.JWT_AUDIENCE,
            )
        except jwt.InvalidTokenError as exc:
            last_error = exc
    raise last_error or jwt.InvalidTokenError("Invalid token")


def hash_token_identifier(identifier: str) -> str:
    return hashlib.sha256(identifier.encode("utf-8")).hexdigest()
