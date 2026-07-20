from unittest.mock import AsyncMock

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
