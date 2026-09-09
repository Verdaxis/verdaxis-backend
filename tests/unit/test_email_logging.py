from unittest.mock import AsyncMock
from uuid import uuid4

import pytest

from app.services import email


class CapturingLogger:
    def __init__(self):
        self.events = []

    def warning(self, event, **metadata):
        self.events.append((event, metadata))

    def info(self, event, **metadata):
        self.events.append((event, metadata))

    def error(self, event, **metadata):
        self.events.append((event, metadata))


@pytest.mark.asyncio
async def test_account_approval_email_links_to_marketplace_and_escapes_name(monkeypatch):
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(email, "_send_email_payload", send)
    monkeypatch.setattr(email.settings, "FRONTEND_URL", "https://staging.verdaxis.exchange/")
    monkeypatch.setattr(email.settings, "EMAIL_FROM", "Verdaxis <approval@example.test>")
    transition_id = uuid4()

    payload = email.build_account_approved_email_payload(
        "approved@example.test",
        "<Merna & Team>",
    )
    assert payload["from"] == "Verdaxis <approval@example.test>"
    assert payload["to"] == ["approved@example.test"]
    assert payload["reply_to"] == "admin@verdaxis.exchange"
    assert email._canonical_email_payload(payload) == payload
    legacy_payload = {key: value for key, value in payload.items() if key != "reply_to"}
    assert email._canonical_email_payload(legacy_payload) == legacy_payload
    assert email._canonical_email_payload({**payload, "reply_to": 123}) is None
    assert payload["subject"] == "Your Verdaxis account has been approved"
    assert 'href="https://staging.verdaxis.exchange/app/marketplace"' in payload["html"]
    assert "&lt;Merna &amp; Team&gt;" in payload["html"]
    assert "<Merna & Team>" not in payload["html"]

    assert await email.send_account_approved_email(payload, transition_id) is True

    send.assert_awaited_once_with(
        payload,
        idempotency_key=f"account-approval/{transition_id}",
    )
    assert send.await_args.kwargs == {
        "idempotency_key": f"account-approval/{transition_id}"
    }


@pytest.mark.asyncio
async def test_low_level_send_forwards_idempotency_key(monkeypatch):
    captured_headers = None
    monkeypatch.setattr(email.settings, "RESEND_API_KEY", "configured-test-key")

    class Response:
        status_code = 201

    class Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, *_args, headers, **_kwargs):
            nonlocal captured_headers
            captured_headers = headers
            return Response()

    monkeypatch.setattr(email.httpx, "AsyncClient", lambda **_kwargs: Client())

    assert await email._send_email(
        "approved@example.test",
        "Approved",
        "<p>Approved</p>",
        idempotency_key="account-approval/test-transition",
    ) is True
    assert captured_headers == {
        "Authorization": "Bearer configured-test-key",
        "Idempotency-Key": "account-approval/test-transition",
    }


@pytest.mark.asyncio
async def test_kyc_approval_email_does_not_claim_full_account_activation(monkeypatch):
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(email, "_send_email", send)

    assert await email.send_kyc_approved_email(
        "approved@example.test",
        "Merna",
    ) is True

    html = send.await_args.args[2]
    assert "identity verification has been approved" in html
    assert "fully active" not in html


@pytest.mark.asyncio
async def test_email_logs_hash_recipient_and_bound_exception_metadata(monkeypatch):
    logger = CapturingLogger()
    monkeypatch.setattr(email, "logger", logger)
    monkeypatch.setattr(email.settings, "RESEND_API_KEY", "configured-test-key")

    class Client:
        async def __aenter__(self):
            return self

        async def __aexit__(self, *_args):
            return None

        async def post(self, *_args, **_kwargs):
            raise RuntimeError("provider leaked alice@example.test and secret body")

    monkeypatch.setattr(email.httpx, "AsyncClient", lambda **_kwargs: Client())
    assert await email._send_email("Alice@Example.Test", "Subject", "<p>body</p>") is False

    event, metadata = logger.events[-1]
    assert event == "email_send_error"
    assert metadata["recipient_hash"] == email._recipient_hash("Alice@Example.Test")
    assert metadata["exception_class"] == "RuntimeError"
    rendered = repr(logger.events)
    assert "Alice@Example.Test" not in rendered
    assert "alice@example.test" not in rendered
    assert "secret body" not in rendered


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "kind, extra, action_path",
    [
        ("verification", {"token": "token&value"}, "/verify-email?token=token%26value"),
        ("password_reset", {"token": "token&value"}, "/reset-password?token=token%26value"),
        ("account_approved", {}, "/app/marketplace"),
        ("kyc_approved", {}, "/login"),
        ("kyc_rejected", {"reason": "<Update & retry>"}, "/kyc"),
        ("referral_invite", {"referral_code": "VDX-SAMPLE"}, "/invite/VDX-SAMPLE"),
        ("signup_alert", {}, "/app/admin/users"),
    ],
)
async def test_automated_emails_share_light_brand_and_support(monkeypatch, kind, extra, action_path):
    send = AsyncMock(return_value=True)
    monkeypatch.setattr(email, "_send_email_payload", send)
    monkeypatch.setattr(email.settings, "FRONTEND_URL", "https://staging.verdaxis.exchange/")
    name = "<Team & Co>"
    if kind == "account_approved":
        payload = email.build_account_approved_email_payload("member@example.com", name)
        await email.send_account_approved_email(payload, uuid4())
    elif kind == "signup_alert":
        await email.send_signup_alert_email(
            user_id=uuid4(), email="member@example.com", name=name,
            role="BUYER", organization="<Shipping & Co>",
        )
    elif kind == "referral_invite":
        await email.send_referral_invite_email("member@example.com", name, **extra)
    else:
        await getattr(email, f"send_{kind}_email")("member@example.com", name, **extra)

    payload = send.await_args.args[0]
    html = payload["html"]
    assert payload["reply_to"] == "admin@verdaxis.exchange"
    assert 'bgcolor="#FFFFFF"' in html and 'bgcolor="#F8FAFC"' in html
    assert 'alt="Verdaxis"' in html
    assert 'mailto:admin@verdaxis.exchange' in html
    assert "Reply to this email" in html
    assert f'href="https://staging.verdaxis.exchange{action_path}"' in html
    assert "&lt;Team &amp; Co&gt;" in html and name not in html
    if kind == "kyc_rejected":
        assert "&lt;Update &amp; retry&gt;" in html
    if kind == "verification":
        assert "expires in 24 hours" in html
    if kind == "password_reset":
        assert "expires in 1 hour" in html
