"""Audit coverage tests for financially/compliance-relevant mutations."""
import ast
from pathlib import Path


EXPECTED_AUDIT_CONSTANTS = {
    "ADMIN_ORGANIZATION_APPROVED": "admin.organization_approved",
    "ADMIN_ORGANIZATION_REJECTED": "admin.organization_rejected",
    "ADMIN_USER_APPROVED": "admin.user_approved",
    "ADMIN_USER_INVITED": "admin.user_invited",
    "ADMIN_USER_REJECTED": "admin.user_rejected",
    "COMMISSION_UPDATED": "commission.updated",
    "INVENTORY_PUBLISHED": "inventory.published",
    "KYC_APPROVED": "kyc.approved",
    "KYC_REJECTED": "kyc.rejected",
    "KYC_SUBMITTED": "kyc.submitted",
    "KYC_MEMBERSHIP_INVALIDATED": "kyc.membership_invalidated",
    "MARKET_ACCESS_INVALIDATED": "market.access_invalidated",
    "MARKET_SUPPORT_AUTHORIZATION_CREATED": "market_support.authorization_created",
    "MARKET_SUPPORT_AUTHORIZATION_REVOKED": "market_support.authorization_revoked",
    "MARKET_SUPPORT_CAPABILITY_GRANTED": "market_support.capability_granted",
    "MARKET_SUPPORT_CAPABILITY_REVOKED": "market_support.capability_revoked",
    "MARKET_SUPPORT_CONTEXT_STARTED": "market_support.context_started",
    "MARKET_SUPPORT_CONTEXT_EXITED": "market_support.context_exited",
    "MARKET_SUPPORT_CONTEXT_REVOKED": "market_support.context_revoked",
    # negotiation.accepted / rfq.accepted are intentionally unregistered:
    # negotiation and RFQ execution are disabled in this release.
    "NEGOTIATION_COUNTERED": "negotiation.countered",
    "NEGOTIATION_CREATED": "negotiation.created",
    "NEGOTIATION_DECLINED": "negotiation.declined",
    "ORDER_CANCELLED": "order.cancelled",
    "ORDER_CREATED": "order.created",
    "ORDER_EXPIRED": "order.expired",
    "ORDER_UPDATED": "order.updated",
    "RFQ_CANCELLED": "rfq.cancelled",
    "RFQ_CREATED": "rfq.created",
    "RFQ_QUOTE_SUBMITTED": "rfq.quote_submitted",
    "RFQ_QUOTE_WITHDRAWN": "rfq.quote_withdrawn",
    "SUBSCRIPTION_UPDATED": "subscription.updated",
    "TRADE_AUTO_MATCHED": "trade.auto_matched",
    "TRADE_CANCELLED": "trade.cancelled",
    "TRADE_CONFIRMED": "trade.confirmed",
    "TRADE_CREATED": "trade.created",
    "TRADE_DECLINED": "trade.declined",
    "TRADE_DELIVERED": "trade.delivered",
    "TRADE_PAID": "trade.paid",
    "USER_PASSWORD_CHANGED": "user.password_changed",
    "USER_PASSWORD_RESET_COMPLETED": "user.password_reset_completed",
    "USER_PASSWORD_RESET_REQUESTED": "user.password_reset_requested",
    "USER_INVITATION_ACCEPTED": "user.invitation_accepted",
    "USER_REGISTERED": "user.registered",
    "ORGANIZATION_JOIN_REQUESTED": "organization.join_requested",
    "ORGANIZATION_JOIN_APPROVED": "organization.join_approved",
    "ORGANIZATION_JOIN_REJECTED": "organization.join_rejected",
}


def _app_source_files() -> list[Path]:
    root = Path(__file__).resolve().parents[2]
    return sorted((root / "app").rglob("*.py"))


def _record_audit_calls() -> list[tuple[Path, ast.Call]]:
    calls: list[tuple[Path, ast.Call]] = []
    for path in _app_source_files():
        tree = ast.parse(path.read_text(), filename=str(path))
        for node in ast.walk(tree):
            if not isinstance(node, ast.Call):
                continue
            func = node.func
            if isinstance(func, ast.Name) and func.id == "record_audit":
                calls.append((path, node))
    return calls


def test_every_action_constant_is_registered():
    from app.services import audit_actions

    constants = {
        name: value
        for name, value in vars(audit_actions).items()
        if name.isupper() and name != "AUDIT_ACTIONS"
    }

    assert constants == EXPECTED_AUDIT_CONSTANTS
    assert audit_actions.AUDIT_ACTIONS == frozenset(EXPECTED_AUDIT_CONSTANTS.values())
    assert set(constants.values()) == audit_actions.AUDIT_ACTIONS


def test_record_audit_action_arguments_use_registered_constants_only():
    from app.services import audit_actions

    constants = {
        name
        for name, value in vars(audit_actions).items()
        if name.isupper() and name != "AUDIT_ACTIONS"
    }
    calls = _record_audit_calls()

    assert calls, "expected at least one record_audit call in app/"

    for path, call in calls:
        action_kw = next((kw for kw in call.keywords if kw.arg == "action"), None)
        assert action_kw is not None, f"{path}:{call.lineno} record_audit missing action="
        assert isinstance(action_kw.value, ast.Name), (
            f"{path}:{call.lineno} action= must be a registered constant identifier"
        )
        assert action_kw.value.id in constants, (
            f"{path}:{call.lineno} action={action_kw.value.id} is not registered"
        )


def test_all_registered_actions_have_a_call_site():
    from app.services import audit_actions

    used_actions = set()
    for _, call in _record_audit_calls():
        action_kw = next((kw for kw in call.keywords if kw.arg == "action"), None)
        if action_kw is not None and isinstance(action_kw.value, ast.Name):
            used_actions.add(getattr(audit_actions, action_kw.value.id))

    assert used_actions == audit_actions.AUDIT_ACTIONS
