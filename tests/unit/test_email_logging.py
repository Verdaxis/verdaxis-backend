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
async def test_account_approval_email_links_to_sign_in_and_escapes_name(monkeypatch):
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
    assert payload["subject"] == "Your Verdaxis account has been approved"
    assert 'href="https://staging.verdaxis.exchange/login"' in payload["html"]
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
