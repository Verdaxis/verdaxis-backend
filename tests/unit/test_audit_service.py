"""Tests for audit_service — record_audit and request context extraction."""
from types import SimpleNamespace
from uuid import uuid4

import pytest

from app.services.audit_service import record_audit, request_audit_context


class _FakeDB:
    def __init__(self):
        self.added = []

    def add(self, obj):
        self.added.append(obj)


@pytest.mark.asyncio
async def test_record_audit_adds_entry_without_committing():
    db = _FakeDB()
    user_id = uuid4()
    resource_id = uuid4()

    entry = await record_audit(
        db,
        user_id=user_id,
        action="trade.confirmed",
        resource_type="trade",
        resource_id=resource_id,
        changes={"status": "CONFIRMED"},
        ip_address="203.0.113.9",
        request_id="req-1",
    )

    assert db.added == [entry]
    assert entry.user_id == user_id
    assert entry.action == "trade.confirmed"
    assert entry.resource_type == "trade"
    assert entry.resource_id == str(resource_id)
    assert entry.changes == {"status": "CONFIRMED"}
    assert entry.ip_address == "203.0.113.9"
    assert entry.request_id == "req-1"


def test_request_audit_context_uses_forwarded_ip_and_header_request_id():
    request = SimpleNamespace(
        headers={"X-Forwarded-For": "6.6.6.6, 203.0.113.9", "X-Request-ID": "rid-7"},
        client=SimpleNamespace(host="127.0.0.1"),
    )
    ctx = request_audit_context(request)
    assert ctx["ip_address"] == "203.0.113.9"
    # request_id_ctx is unset outside the middleware, so the header wins
    assert ctx["request_id"] == "rid-7"
