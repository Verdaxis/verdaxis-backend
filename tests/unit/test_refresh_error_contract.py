"""Stable machine-readable refresh authentication error contract."""

from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import UUID, uuid4

import jwt
import pytest
from fastapi import HTTPException, Request, Response

from app.routers import auth_simple
from app.core.security import create_refresh_token, decode_token, hash_token_identifier
from app.models.refresh_session import RefreshSession
from app.models.user import UserStatus


EXPECTED_TERMINAL_REFRESH_CODES = frozenset(
    {
        "REFRESH_TOKEN_MISSING",
        "REFRESH_TOKEN_EXPIRED",
        "REFRESH_TOKEN_INVALID",
        "REFRESH_SESSION_REVOKED",
        "REFRESH_TOKEN_REPLAYED",
        "REFRESH_ACCOUNT_INACTIVE",
        "REFRESH_PASSWORD_CHANGED",
        "REFRESH_DEVICE_REQUIRED",
    }
)


def _request(cookie_value: str | None = None) -> Request:
    headers = []
    if cookie_value is not None:
        headers.append(
            (
                b"cookie",
                (
                    f"{auth_simple.REFRESH_COOKIE_NAME}={cookie_value}; "
                    f"{auth_simple.DEVICE_SESSION_COOKIE_NAME}={'D' * 43}"
                ).encode(),
            )
        )
        headers.append((b"origin", b"https://test"))
    return Request({"type": "http", "method": "POST", "path": "/auth/refresh", "headers": headers})


def test_terminal_refresh_code_allowlist_is_explicit_and_closed():
    assert auth_simple.TERMINAL_REFRESH_ERROR_CODES == EXPECTED_TERMINAL_REFRESH_CODES


@pytest.mark.parametrize("code", sorted(EXPECTED_TERMINAL_REFRESH_CODES))
def test_terminal_refresh_error_keeps_code_and_human_message_separate(code):
    error = auth_simple._terminal_refresh_error(code, "Human-readable guidance")

    assert error.status_code == 401
    assert error.detail == {"code": code, "message": "Human-readable guidance"}


@pytest.mark.asyncio
async def test_missing_refresh_token_returns_stable_terminal_code():
    with pytest.raises(HTTPException) as exc_info:
        await auth_simple.refresh_tokens(
            request=_request(),
            response=Response(),
            body=None,
            db=AsyncMock(),
        )

    assert exc_info.value.status_code == 401
    assert exc_info.value.detail["code"] == "REFRESH_TOKEN_MISSING"


@pytest.mark.parametrize(
    ("decode_error", "expected_code"),
    [
        (jwt.ExpiredSignatureError(), "REFRESH_TOKEN_EXPIRED"),
        (jwt.InvalidTokenError(), "REFRESH_TOKEN_INVALID"),
    ],
)
@pytest.mark.asyncio
async def test_bad_refresh_token_returns_specific_stable_code(monkeypatch, decode_error, expected_code):
    monkeypatch.setattr(auth_simple, "decode_token", lambda _token: (_ for _ in ()).throw(decode_error))

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple.refresh_tokens(
            request=_request("redacted"),
            response=Response(),
            body=None,
            db=AsyncMock(),
        )

    assert exc_info.value.status_code == 401
    assert exc_info.value.detail["code"] == expected_code


@pytest.mark.asyncio
async def test_malformed_refresh_subject_returns_stable_invalid_code(monkeypatch):
    monkeypatch.setattr(
        auth_simple,
        "decode_token",
        lambda _token: {"type": "refresh", "sub": ["not", "a", "uuid"]},
    )

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple.refresh_tokens(
            request=_request("redacted"),
            response=Response(),
            body=None,
            db=AsyncMock(),
        )

    assert exc_info.value.status_code == 401
    assert exc_info.value.detail["code"] == "REFRESH_TOKEN_INVALID"


def test_rotation_in_progress_remains_the_only_transient_refresh_conflict():
    assert auth_simple.REFRESH_TRANSIENT_ERROR_CODES == frozenset({"REFRESH_ROTATION_IN_PROGRESS"})


def _scalar_result(value):
    result = MagicMock()
    result.scalar_one_or_none.return_value = value
    return result


@pytest.mark.asyncio
async def test_inactive_refresh_account_returns_stable_terminal_code():
    user_id = uuid4()
    token = create_refresh_token(str(user_id))
    user = SimpleNamespace(id=user_id, status=UserStatus.REJECTED, password_changed_at=None)
    db = AsyncMock()
    db.execute.return_value = _scalar_result(user)

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple.refresh_tokens(
            request=_request(token), response=Response(), body=None, db=db
        )

    assert exc_info.value.detail["code"] == "REFRESH_ACCOUNT_INACTIVE"


@pytest.mark.asyncio
async def test_password_changed_refresh_returns_stable_terminal_code():
    user_id = uuid4()
    token = create_refresh_token(str(user_id))
    user = SimpleNamespace(
        id=user_id,
        status=UserStatus.APPROVED,
        password_changed_at=datetime.now(UTC) + timedelta(microseconds=1),
    )
    db = AsyncMock()
    db.execute.return_value = _scalar_result(user)

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple.refresh_tokens(
            request=_request(token), response=Response(), body=None, db=db
        )

    assert exc_info.value.detail["code"] == "REFRESH_PASSWORD_CHANGED"


@pytest.mark.asyncio
async def test_revoked_refresh_session_returns_stable_terminal_code():
    user_id = uuid4()
    token = create_refresh_token(str(user_id))
    payload = decode_token(token)
    session = RefreshSession(
        user_id=user_id,
        family_id=UUID(payload["family_id"]),
        jti_hash=hash_token_identifier(payload["jti"]),
        expires_at=datetime.fromtimestamp(payload["exp"], tz=UTC),
        device_id_hash="a" * 64,
        revoked=True,
    )
    db = AsyncMock()
    db.execute.side_effect = [
        _scalar_result(SimpleNamespace(id=user_id)),
        _scalar_result(session),
    ]

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple._rotate_refresh_session(
            db,
            user_id,
            payload,
            create_refresh_token(str(user_id), family_id=payload["family_id"]),
            device_id_hash="a" * 64,
        )

    assert exc_info.value.detail["code"] == "REFRESH_SESSION_REVOKED"


@pytest.mark.asyncio
async def test_post_grace_refresh_replay_returns_code_and_revokes_family():
    user_id = uuid4()
    token = create_refresh_token(str(user_id))
    payload = decode_token(token)
    session = RefreshSession(
        user_id=user_id,
        family_id=UUID(payload["family_id"]),
        jti_hash=hash_token_identifier(payload["jti"]),
        expires_at=datetime.fromtimestamp(payload["exp"], tz=UTC),
        device_id_hash="a" * 64,
        revoked=True,
        replaced_by_jti_hash="a" * 64,
        rotation_grace_until=datetime.now(UTC) - timedelta(seconds=1),
    )
    db = AsyncMock()
    db.execute.side_effect = [
        _scalar_result(SimpleNamespace(id=user_id)),
        _scalar_result(session),
        MagicMock(scalars=lambda: SimpleNamespace(first=lambda: SimpleNamespace(id=uuid4()))),
        MagicMock(rowcount=2),
    ]

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple._rotate_refresh_session(
            db,
            user_id,
            payload,
            create_refresh_token(str(user_id), family_id=payload["family_id"]),
            device_id_hash="a" * 64,
        )

    assert exc_info.value.detail["code"] == "REFRESH_TOKEN_REPLAYED"
    db.commit.assert_awaited_once()


@pytest.mark.asyncio
async def test_revoked_family_during_grace_is_terminal_not_rotation_in_progress():
    user_id = uuid4()
    token = create_refresh_token(str(user_id))
    payload = decode_token(token)
    session = RefreshSession(
        user_id=user_id,
        family_id=UUID(payload["family_id"]),
        jti_hash=hash_token_identifier(payload["jti"]),
        expires_at=datetime.fromtimestamp(payload["exp"], tz=UTC),
        device_id_hash="a" * 64,
        revoked=True,
        replaced_by_jti_hash="a" * 64,
        rotation_grace_until=datetime.now(UTC) + timedelta(seconds=2),
    )
    no_active_family = MagicMock()
    no_active_family.scalars.return_value.first.return_value = None
    db = AsyncMock()
    db.execute.side_effect = [
        _scalar_result(SimpleNamespace(id=user_id)),
        _scalar_result(session),
        no_active_family,
    ]

    with pytest.raises(HTTPException) as exc_info:
        await auth_simple._rotate_refresh_session(
            db,
            user_id,
            payload,
            create_refresh_token(str(user_id), family_id=payload["family_id"]),
            device_id_hash="a" * 64,
        )

    assert exc_info.value.status_code == 401
    assert exc_info.value.detail["code"] == "REFRESH_SESSION_REVOKED"
