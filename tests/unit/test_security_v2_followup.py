"""Regression contracts from the second security-v2 adversarial review."""

from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import uuid4

import pytest
from fastapi import HTTPException, Request, Response
from sqlalchemy.exc import DBAPIError

from app.config import settings
from app.core.security import create_access_token, decode_token
from app.middleware.execution import require_execution_eligible_user
from app.models.negotiation import Negotiation, NegotiationRound
from app.models.refresh_session import RefreshSession
from app.models.rfq import RFQStatus
from app.models.user import UserRole, UserStatus
from app.schemas.user import PasswordChangeRequest
from app.routers import auth_simple, kyc, negotiations, orderbook, rfq, trades
from app.services.email_domains import registration_organization_domain
from app.services.execution_policy import execution_party_is_eligible


ROOT = Path(__file__).resolve().parents[2]


def _eligible_user(*, organization_id=None):
    return SimpleNamespace(
        id=uuid4(),
        organization_id=organization_id or uuid4(),
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
        kyc_status="APPROVED",
        kyc_external_evidence_reference="external-case-123",
        kyc_review_note="Corporate registry and identity evidence reviewed externally.",
        kyc_reviewed_by=uuid4(),
        kyc_reviewed_at=datetime.now(UTC),
    )


def _approved_org(org_id):
    return SimpleNamespace(id=org_id, verification_status="APPROVED")


def _scalar_result(value):
    result = MagicMock()
    result.scalar_one_or_none.return_value = value
    return result


def test_public_and_disposable_domains_never_form_tenant_boundaries():
    for email in (
        "person@gmail.com",
        "person@mailinator.com",
        "person@guerrillamail.com",
    ):
        assert registration_organization_domain(email) is None


@pytest.mark.asyncio
async def test_anonymous_rfq_response_omits_stable_buyer_tenant_identity():
    buyer_org_id = uuid4()
    anonymous_rfq = SimpleNamespace(
        id=uuid4(),
        buyer_org_id=buyer_org_id,
        product_id=uuid4(),
        delivery_point_id=None,
        quantity_mt=100,
        target_price_per_mt=None,
        availability_window="SPOT",
        notes=None,
        is_anonymous=True,
        status=RFQStatus.OPEN,
        expires_at=datetime.now(UTC),
        created_at=datetime.now(UTC),
        quotes=[],
    )
    db = AsyncMock()
    db.execute.side_effect = [
        _scalar_result("Identifying Buyer Name"),
        _scalar_result("Methanol"),
    ]

    response = await rfq._build_rfq_response(
        db,
        anonymous_rfq,
        viewer_org_id=uuid4(),
    )

    assert response.buyer_org_id is None
    assert response.buyer_org_name == "Anonymous"


def test_domain_dataset_failure_disables_all_domain_attachment(monkeypatch):
    from app.services import email_domains

    monkeypatch.setattr(email_domains, "DOMAIN_DATASETS_AVAILABLE", False, raising=False)
    assert email_domains.registration_organization_domain("person@company.example") is None


def test_join_review_has_distinct_admin_routes_and_no_role_switch_debug_route():
    paths = {route.path for route in auth_simple.router.routes}
    assert "/auth/organization-joins" in paths
    assert "/auth/organization-joins/{join_request_id}/approve" in paths
    assert "/auth/organization-joins/{join_request_id}/reject" in paths
    assert all("switch-role" not in path for path in paths)


def test_user_and_org_approval_source_do_not_transition_join_requests():
    source = (ROOT / "app" / "routers" / "auth_simple.py").read_text()
    user_approval = source[source.index("async def approve_user("):source.index("async def approve_organization(")]
    org_approval = source[source.index("async def approve_organization("):source.index("async def reject_organization(")]
    assert "ORGANIZATION_JOIN_APPROVED" not in user_approval
    assert "ORGANIZATION_JOIN_APPROVED" not in org_approval


@pytest.mark.asyncio
async def test_execution_party_requires_exact_membership_while_kyc_is_advisory():
    org_id = uuid4()
    user = _eligible_user(organization_id=org_id)
    assert await execution_party_is_eligible(
        SimpleNamespace(), user=user, organization=_approved_org(org_id)
    ) is True

    assert await execution_party_is_eligible(
        SimpleNamespace(), user=user, organization=_approved_org(uuid4())
    ) is False

    user.kyc_external_evidence_reference = None
    assert await execution_party_is_eligible(
        SimpleNamespace(), user=user, organization=_approved_org(org_id)
    ) is True


@pytest.mark.asyncio
async def test_admin_cannot_fall_through_execution_gate():
    admin = _eligible_user()
    admin.role = UserRole.ADMIN
    with pytest.raises(HTTPException) as exc_info:
        await require_execution_eligible_user(admin, AsyncMock())
    assert exc_info.value.status_code == 403


def test_negotiations_store_both_party_users_acceptor_and_round_proposer():
    assert hasattr(Negotiation, "initiator_user_id")
    assert hasattr(Negotiation, "counterparty_user_id")
    assert hasattr(Negotiation, "accepted_by_user_id")
    assert hasattr(NegotiationRound, "proposer_user_id")


def test_refresh_expiry_is_indexed():
    indexed_columns = {
        column.name
        for index in RefreshSession.__table__.indexes
        for column in index.columns
    }
    assert "expires_at" in indexed_columns


def test_tokens_carry_high_resolution_issuance_for_precise_password_cutoff():
    payload = decode_token(create_access_token("user"))
    assert isinstance(payload.get("iat_us"), int)


@pytest.mark.parametrize(
    ("environment", "secure_expected"),
    [("production", True), ("staging", True), ("development", False)],
)
def test_refresh_cookie_security_is_environment_safe(monkeypatch, environment, secure_expected):
    monkeypatch.setattr(settings, "ENVIRONMENT", environment)
    response = Response()
    auth_simple._set_refresh_cookie(response, "opaque-token")
    header = response.headers["set-cookie"]

    assert ("Secure" in header) is secure_expected
    assert "HttpOnly" in header
    assert "SameSite=lax" in header
    assert "Path=/api/auth" in header


def test_refresh_cookie_clear_uses_same_security_attributes(monkeypatch):
    monkeypatch.setattr(settings, "ENVIRONMENT", "staging")
    response = Response()
    auth_simple._clear_refresh_cookie(response)
    header = response.headers["set-cookie"]
    assert "Secure" in header
    assert "HttpOnly" in header
    assert "SameSite=lax" in header
    assert "Path=/api/auth" in header
    assert "Max-Age=0" in header


def _direct_route_dependencies(router, path: str, method: str) -> set[object]:
    route = next(
        route
        for route in router.routes
        if route.path == path and method in getattr(route, "methods", set())
    )
    return {dependency.call for dependency in route.dependant.dependencies}


def test_only_owner_cleanup_routes_use_inactive_account_authentication():
    cleanup_routes = (
        (orderbook.router, "/orderbook/{order_id}", "DELETE"),
        (rfq.router, "/rfq/{rfq_id}/cancel", "POST"),
        (negotiations.router, "/negotiations/{negotiation_id}/decline", "POST"),
        (trades.router, "/trades/{trade_id}/decline", "PUT"),
    )
    for router, path, method in cleanup_routes:
        dependencies = _direct_route_dependencies(router, path, method)
        assert auth_simple.get_authenticated_user in dependencies
        assert auth_simple.get_current_user not in dependencies


@pytest.mark.asyncio
async def test_rejected_user_remains_authenticated_for_exact_owner_cleanup():
    user = _eligible_user()
    user.status = UserStatus.REJECTED
    user.password_changed_at = None
    token = create_access_token(user.id)
    result = MagicMock()
    result.scalar_one_or_none.return_value = user
    db = AsyncMock()
    db.execute.return_value = result
    request = Request(
        {
            "type": "http",
            "method": "DELETE",
            "path": "/api/orderbook/example",
            "query_string": b"",
            "headers": [],
        }
    )

    authenticated = await auth_simple.get_authenticated_user(request, token, db)

    assert authenticated is user


@pytest.mark.asyncio
async def test_logout_discloses_sanitized_revocation_failure_and_clears_cookie():
    token = auth_simple.create_refresh_token(str(uuid4()))
    db = AsyncMock()
    db.execute.side_effect = DBAPIError("statement", {}, RuntimeError("secret database detail"))
    request = SimpleNamespace(
        cookies={auth_simple.REFRESH_COOKIE_NAME: token},
        headers={"origin": "https://test"},
    )
    response = Response()

    result = await auth_simple.logout(request=request, response=response, db=db)

    assert result.status_code == 503
    assert b"REVOCATION_FAILED" in result.body
    assert b"secret database detail" not in result.body
    assert "Max-Age=0" in result.headers["set-cookie"]


@pytest.mark.asyncio
async def test_password_change_persistence_failure_never_sets_untracked_cookie():
    user = _eligible_user()
    user.password_hash = "old-hash"
    user.must_change_password = False
    result = MagicMock()
    result.scalar_one_or_none.return_value = user
    db = AsyncMock()
    db.add = MagicMock()
    db.execute.side_effect = [result, MagicMock(rowcount=1)]
    db.commit.side_effect = DBAPIError("statement", {}, RuntimeError("secret database detail"))
    request = Request(
        {
            "type": "http",
            "method": "PUT",
            "path": "/api/auth/me/password",
            "query_string": b"",
            "headers": [],
            "client": ("127.0.0.1", 1234),
            "scheme": "https",
            "server": ("test", 443),
        }
    )
    response = Response()

    with (
        patch.object(auth_simple, "verify_password", return_value=True),
        patch.object(auth_simple, "get_password_hash", return_value="new-hash"),
        pytest.raises(HTTPException) as exc_info,
    ):
        await auth_simple.change_password(
            request=request,
            response=response,
            payload=PasswordChangeRequest(
                current_password="current-password",
                new_password="replacement-password",
            ),
            current_user=user,
            db=db,
        )

    assert exc_info.value.status_code == 503
    assert "set-cookie" not in response.headers
    db.rollback.assert_awaited_once()


def test_kyc_approval_requires_external_evidence_reference_and_review_note():
    body_type = getattr(kyc, "AdminApproveBody", None)
    assert body_type is not None
    with pytest.raises(Exception):
        body_type()
    body = body_type(
        external_evidence_reference="external-case-123",
        review_note="Identity and company evidence reviewed in the external case system.",
    )
    assert body.external_evidence_reference == "external-case-123"


@pytest.mark.parametrize(
    "body_factory",
    [
        lambda: kyc.AdminApproveBody(
            external_evidence_reference="   ",
            review_note="Identity and company evidence reviewed externally.",
        ),
        lambda: kyc.AdminApproveBody(
            external_evidence_reference="external-case-123",
            review_note="          ",
        ),
        lambda: auth_simple.JoinReviewBody(review_note="   "),
    ],
)
def test_admin_review_evidence_fields_reject_whitespace_only_values(body_factory):
    with pytest.raises(Exception):
        body_factory()


def test_auth_maintenance_is_independent_from_news_and_plaintext_fallback_is_removed():
    news_cli = (ROOT / "app" / "cli" / "refresh_news.py").read_text()
    auth_cli = ROOT / "app" / "cli" / "auth_maintenance.py"
    auth_service = ROOT / "app" / "services" / "auth_maintenance.py"
    auth_source = (ROOT / "app" / "routers" / "auth_simple.py").read_text()
    identity_migration = (ROOT / "alembic" / "versions" / "sec_20260720_identity_hardening.py").read_text()
    boundary_migration = (ROOT / "alembic" / "versions" / "sec_20260720_admission_boundaries.py").read_text()

    assert auth_cli.exists()
    assert auth_service.exists()
    assert "cleanup_pending_registrations" not in news_cli
    assert "email_verification_token == token" not in auth_source
    assert "verdaxis_sync_legacy_verification_token" in identity_migration
    assert "sha256(convert_to(NEW.email_verification_token, 'UTF8'))" in identity_migration
    assert "unbound_legacy_tokens" in boundary_migration
    assert 'drop_column("users", "email_verification_token")' in boundary_migration


def test_production_units_disable_uvicorn_access_log_and_auth_timer_is_deployable():
    for name in ("verdaxis-backend.service", "verdaxis-backend-staging.service"):
        unit = (ROOT / "deploy" / "systemd" / name).read_text()
        assert "--no-access-log" in unit
    for name in (
        "verdaxis-auth-maintenance.service",
        "verdaxis-auth-maintenance.timer",
        "verdaxis-auth-maintenance-staging.service",
        "verdaxis-auth-maintenance-staging.timer",
    ):
        assert (ROOT / "deploy" / "systemd" / name).exists()


def test_inventory_errors_are_sanitized_and_preflight_reports_all_legacy_provenance():
    inventory_source = (ROOT / "app" / "routers" / "inventory.py").read_text()
    preflight_source = (ROOT / "scripts" / "security_preflight.py").read_text()
    assert "detail=f\"Failed to create inventory item" not in inventory_source
    assert "exc_info=True" not in inventory_source
    assert "legacy_negotiations_without_user_provenance" in preflight_source
    assert "approved_kyc_without_external_review_evidence" in preflight_source


def test_security_migrations_remain_linear_without_empty_merge_revision():
    identity = ROOT / "alembic" / "versions" / "sec_20260720_identity_hardening.py"
    boundaries = ROOT / "alembic" / "versions" / "sec_20260720_admission_boundaries.py"
    assert 'revision: str = "sec_20260720_identity"' in identity.read_text()
    assert 'down_revision: Union[str, None] = "sec_20260720_identity"' in boundaries.read_text()
    assert "pass\n" not in boundaries.read_text()
