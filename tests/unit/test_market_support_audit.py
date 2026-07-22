from uuid import uuid4

import pytest

from app.services.audit_service import record_audit
from app.services.market_support_context import (
    MarketSupportActionContext,
    market_support_action_context,
)


class _FakeSession:
    def __init__(self):
        self.entries = []

    def add(self, entry):
        self.entries.append(entry)


@pytest.mark.asyncio
async def test_delegated_action_preserves_admin_as_audit_actor():
    principal_id = uuid4()
    admin_id = uuid4()
    context = MarketSupportActionContext(
        actor_user_id=admin_id,
        target_organization_id=uuid4(),
        accountable_user_id=principal_id,
        support_authorization_id=uuid4(),
        operation="CREATE",
        reason_code="CUSTOMER_ONBOARDING",
        support_case_reference="VDX-100",
        idempotency_key="key-1",
        request_hash="a" * 64,
        support_version=1,
    )
    session = _FakeSession()
    with market_support_action_context(context):
        entry = await record_audit(
            session,
            user_id=principal_id,
            action="order.created",
            resource_type="order",
            resource_id=str(uuid4()),
            changes={"status": "OPEN"},
        )
    assert entry.user_id == admin_id
    assert entry.changes["market_support"]["accountable_user_id"] == str(
        principal_id
    )
    assert entry.changes["market_support"]["submission_method"] == (
        "VERDAXIS_ASSISTED"
    )
