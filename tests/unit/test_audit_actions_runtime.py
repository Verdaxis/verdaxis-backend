"""Runtime checks for registered audit actions."""
from uuid import uuid4

import pytest

from app.models.audit import AuditLog
from app.services import audit_actions
from app.services.audit_service import record_audit


class _FakeDB:
    def __init__(self):
        self.added = []

    def add(self, obj):
        self.added.append(obj)


@pytest.mark.asyncio
@pytest.mark.parametrize("action", sorted(audit_actions.AUDIT_ACTIONS))
async def test_registered_action_records_exactly_one_audit_row(action):
    db = _FakeDB()

    await record_audit(
        db,
        user_id=uuid4(),
        action=action,
        resource_type="test",
        resource_id=str(uuid4()),
        changes={"status": {"from": "before", "to": "after"}},
        ip_address="203.0.113.9",
        request_id="req-1",
    )

    audit_rows = [entry for entry in db.added if isinstance(entry, AuditLog)]
    assert len(audit_rows) == 1
    assert audit_rows[0].action == action
