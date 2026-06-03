import pytest
from fastapi import HTTPException, status

from app.routers.monitor import _assert_canary_email, _require_monitor_token


def test_canary_cleanup_accepts_only_monitor_email_domain():
    assert _assert_canary_email("canary+prod-123@prod-123.canary.verdaxis.exchange") == "prod-123.canary.verdaxis.exchange"


@pytest.mark.parametrize(
    "email",
    [
        "user@example.com",
        "canary@example.com",
        "canary+prod@verdaxis.exchange",
        "canary+prod@prod-123.verdaxis.invalid",
    ],
)
def test_canary_cleanup_rejects_non_canary_emails(email):
    with pytest.raises(HTTPException) as exc_info:
        _assert_canary_email(email)
    assert exc_info.value.status_code == 422


def test_monitor_token_required_when_configured(monkeypatch):
    monkeypatch.setattr("app.routers.monitor.settings.MONITOR_TOKEN", "secret")

    with pytest.raises(HTTPException) as missing:
        _require_monitor_token(None)
    with pytest.raises(HTTPException) as invalid:
        _require_monitor_token("wrong")

    assert missing.value.status_code == status.HTTP_401_UNAUTHORIZED
    assert invalid.value.status_code == status.HTTP_401_UNAUTHORIZED


def test_monitor_token_hidden_when_disabled(monkeypatch):
    monkeypatch.setattr("app.routers.monitor.settings.MONITOR_TOKEN", None)

    with pytest.raises(HTTPException) as exc_info:
        _require_monitor_token("secret")

    assert exc_info.value.status_code == status.HTTP_404_NOT_FOUND
