from unittest.mock import Mock
from pathlib import Path

from app.routers.auth_simple import _should_skip_verification_email_for_canary
from app.services.monitor_canary import get_monitor_canary_domain, is_monitor_canary_email


def test_canary_email_pattern_is_narrow():
    assert is_monitor_canary_email("canary+prod-123@prod-123.canary.verdaxis.exchange") is True
    assert get_monitor_canary_domain("canary+prod-123@prod-123.canary.verdaxis.exchange") == "prod-123.canary.verdaxis.exchange"
    assert is_monitor_canary_email("user@prod-123.canary.verdaxis.exchange") is False
    assert is_monitor_canary_email("canary+prod@verdaxis.exchange") is False


def test_monitor_token_skips_email_for_canary(monkeypatch):
    monkeypatch.setattr("app.routers.auth_simple.settings.MONITOR_TOKEN", "secret")
    request = Mock()
    request.headers = {"X-Monitor-Token": "secret"}

    assert _should_skip_verification_email_for_canary(
        request,
        "canary+prod-123@prod-123.canary.verdaxis.exchange",
    ) is True


def test_monitor_token_does_not_skip_real_user_email(monkeypatch):
    monkeypatch.setattr("app.routers.auth_simple.settings.MONITOR_TOKEN", "secret")
    request = Mock()
    request.headers = {"X-Monitor-Token": "secret"}

    assert _should_skip_verification_email_for_canary(request, "real@example.com") is False


def test_canary_email_does_not_skip_without_monitor_token(monkeypatch):
    monkeypatch.setattr("app.routers.auth_simple.settings.MONITOR_TOKEN", "secret")
    request = Mock()
    request.headers = {}

    assert _should_skip_verification_email_for_canary(
        request,
        "canary+prod-123@prod-123.canary.verdaxis.exchange",
    ) is False


def test_registration_canary_remains_cleanup_safe():
    source = Path("app/routers/auth_simple.py").read_text()
    registration = source.split("async def register_with_org(", 1)[1].split(
        "# ---------------------------------------------------------------------------\n# Email verification",
        1,
    )[0]

    assert "if not is_monitor_canary:" in registration
    guarded = registration.split("if not is_monitor_canary:", 1)[1]
    assert "OrganizationJoinRequest(" in guarded
    assert "await record_audit(" in guarded
