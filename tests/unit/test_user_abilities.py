"""User abilities payload: contract, per-user outcomes, and endpoint agreement.

The agreement tests are the point of this file. Each ability is computed by the
same predicate the enforcing endpoint calls, so for every user profile the real
endpoint (or dependency) and the abilities payload must reach the same verdict.
"""
from __future__ import annotations

from contextlib import asynccontextmanager
from datetime import UTC, datetime, timedelta
from types import SimpleNamespace
from uuid import uuid4

import pytest
from app.routers.abilities import router as abilities_router
from app.services.abilities import REASON_MESSAGES, SCHEMA_VERSION, build_abilities
from app.services.action_policy import (
    AbilityId,
    ReasonCode,
    allowed_roles,
    hit_order_role_denial,
    place_order_role_denial,
    role_may,
)
from fastapi import FastAPI, HTTPException
from httpx import ASGITransport, AsyncClient

from app.database import get_db
from app.middleware.execution import require_execution_eligible_user
from app.middleware.rbac import require_role
from app.models.market_support import (
    MarketSupportCapability,
    StaffCapabilityAssignment,
)
from app.models.orderbook import OrderBookStatus, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    OrgType,
    User,
    UserRole,
    UserStatus,
)
from app.routers import inventory as inventory_module
from app.routers import market_support as market_support_module
from app.routers import orderbook as orderbook_module
from app.routers import trades as trades_module
from app.routers.auth_simple import AUTHENTICATED_SHARED_LOCK_PATHS, get_current_user
from app.routers.rfq import router as rfq_router
from app.services.execution_policy import (
    execution_party_denial,
    execution_party_is_eligible,
)
from app.services.market_admission import (
    assert_market_participant_provenance,
    participant_provenance_eligible,
)
from app.services.market_support import capability_assignment_is_active
from app.services.provenance import (
    execution_provenance_compatible,
    provenance_may_execute,
)
from app.services.request_party import legacy_admin_workspace_mutation_denial

A = AbilityId
R = ReasonCode

ADMIN_ABILITIES = (
    A.VIEW_ADMIN_ANALYTICS,
    A.VIEW_AUDIT_LOG,
    A.VIEW_USER_ACTIVITY,
    A.VIEW_FEEDBACK,
    A.MANAGE_SUBSCRIPTIONS,
    A.MANAGE_ACCOUNTS,
    A.REVIEW_KYC,
    A.MANAGE_COMMISSIONS,
)
MARKET_SUPPORT_ABILITIES = (A.MARKET_SUPPORT_ACCESS, A.MARKET_SUPPORT_ASSISTED_ORDERS)
SUPPLIER_ONLY = (A.PLACE_ASK_ORDER, A.HIT_BID_ORDER, A.PAY_TRADE, A.MANAGE_INVENTORY, A.PUBLISH_INVENTORY, A.QUOTE_RFQ)
BUYER_ONLY = (A.PLACE_BID_ORDER, A.HIT_ASK_ORDER, A.CREATE_RFQ)
ANY_PARTICIPANT = (A.AMEND_ORDER, A.CONFIRM_TRADE, A.DELIVER_TRADE)
EXECUTION_GATED = tuple(
    ability
    for ability in A
    if ability not in ADMIN_ABILITIES + MARKET_SUPPORT_ABILITIES + (A.CANCEL_ORDER,)
)


# ---------------------------------------------------------------------------
# Fixtures: user profiles
# ---------------------------------------------------------------------------


def make_profile(kind: str) -> tuple[User, Organization | None]:
    """Return a user and the organization row the user's endpoint would load."""
    role = {"supplier": UserRole.SUPPLIER, "admin": UserRole.ADMIN}.get(kind, UserRole.BUYER)
    if kind.startswith("supplier"):
        role = UserRole.SUPPLIER
    organization_id = uuid4()
    user = User(
        id=uuid4(),
        email=f"{kind}@example.com",
        role=role,
        status=UserStatus.APPROVED,
        organization_id=organization_id,
        email_verified=True,
        must_change_password=False,
        kyc_status="APPROVED",
        kyc_organization_id=None,
    )
    organization = Organization(
        id=organization_id,
        name="Acme",
        type=OrgType.FUEL_SUPPLIER if role == UserRole.SUPPLIER else OrgType.SHIPPING_LINE,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    if kind == "org_pending":
        organization.verification_status = "PENDING"
    elif kind == "kyc_rejected":
        user.kyc_status = "REJECTED"
    elif kind == "kyc_other_org":
        user.kyc_organization_id = uuid4()
    elif kind in {"unknown_provenance", "supplier_unknown_provenance"}:
        organization.provenance = OrganizationProvenance.UNKNOWN
    elif kind == "email_unverified":
        user.email_verified = False
    elif kind == "password_change":
        user.must_change_password = True
    elif kind == "role_unassigned":
        user.role = None
    elif kind == "no_organization":
        user.organization_id = None
        organization = None
    return user, organization


PROFILE_KINDS = (
    "buyer",
    "supplier",
    "admin",
    "org_pending",
    "kyc_rejected",
    "kyc_other_org",
    "unknown_provenance",
    "supplier_unknown_provenance",
    "email_unverified",
    "password_change",
    "role_unassigned",
    "no_organization",
)


def by_action(user, organization, capabilities=frozenset()):
    return {a.action: a for a in build_abilities(user, organization, capabilities)}


class FakeResult:
    def __init__(self, organization, assignments=()):
        self._organization = organization
        self._assignments = list(assignments)

    def scalar_one_or_none(self):
        return self._organization

    def scalars(self):
        return SimpleNamespace(first=lambda: None, all=lambda: list(self._assignments))

    def unique(self):
        return self

    def one_or_none(self):
        return None


class FakeDb:
    """Answers every select with the user's organization (or no rows)."""

    def __init__(self, organization, assignments=()):
        self._organization = organization
        self._assignments = assignments

    async def execute(self, *_args, **_kwargs):
        return FakeResult(self._organization, self._assignments)

    async def rollback(self):
        return None


class PassedGate(Exception):
    """Raised by a stub placed immediately after an endpoint's access checks."""


async def gate_outcome(coro) -> str:
    """Return 'denied' for an HTTP 403, 'passed' when execution reached the stub."""
    try:
        await coro
    except HTTPException as error:
        if error.status_code in (403, 410):
            return "denied"
        return "passed"
    except PassedGate:
        return "passed"
    return "passed"


# ---------------------------------------------------------------------------
# Payload contract and outcomes per user kind
# ---------------------------------------------------------------------------


def test_every_ability_is_reported_once_in_declaration_order():
    user, organization = make_profile("buyer")
    abilities = build_abilities(user, organization)
    assert [a.action for a in abilities] == list(AbilityId)
    assert len({a.action for a in abilities}) == len(abilities)


def test_ids_and_codes_are_snake_case_and_upper_case_and_every_code_has_a_message():
    assert all(a.value == a.value.lower() and " " not in a.value for a in AbilityId)
    assert all(c.value == c.name and c.value == c.value.upper() for c in ReasonCode)
    assert set(REASON_MESSAGES) == set(ReasonCode)


def test_allowed_abilities_carry_no_reason_and_denied_abilities_always_do():
    for kind in PROFILE_KINDS:
        user, organization = make_profile(kind)
        for ability in build_abilities(user, organization):
            if ability.allowed:
                assert ability.reason_code is None and ability.reason is None
            else:
                assert ability.reason_code is not None
                assert ability.reason == REASON_MESSAGES[ability.reason_code]


def verdicts(kind, capabilities=frozenset()):
    user, organization = make_profile(kind)
    return {
        action: ability.reason_code
        for action, ability in by_action(user, organization, capabilities).items()
    }


def expected(allowed, denied_with, default_denial=None):
    """Build the full verdict map: allowed -> None, everything else -> a code."""
    result = {}
    for ability in AbilityId:
        if ability in allowed:
            result[ability] = None
        elif ability in denied_with:
            result[ability] = denied_with[ability]
        else:
            result[ability] = default_denial
    return result


def test_buyer_payload():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    admin_denied = {a: R.ADMIN_ROLE_REQUIRED for a in ADMIN_ABILITIES}
    supplier_denied = {a: R.SUPPLIER_ROLE_REQUIRED for a in SUPPLIER_ONLY}
    assert verdicts("buyer") == expected(
        allowed=set(BUYER_ONLY + ANY_PARTICIPANT + (A.CANCEL_ORDER,)),
        denied_with={**market_support_off, **admin_denied, **supplier_denied},
    )


def test_supplier_payload():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    admin_denied = {a: R.ADMIN_ROLE_REQUIRED for a in ADMIN_ABILITIES}
    buyer_denied = {a: R.BUYER_ROLE_REQUIRED for a in BUYER_ONLY}
    assert verdicts("supplier") == expected(
        allowed=set(SUPPLIER_ONLY + ANY_PARTICIPANT + (A.CANCEL_ORDER,)),
        denied_with={**market_support_off, **admin_denied, **buyer_denied},
    )


def test_admin_payload_with_market_support_disabled():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    execution_denied = {a: R.ADMIN_CANNOT_EXECUTE for a in EXECUTION_GATED}
    assert verdicts("admin") == expected(
        allowed=set(ADMIN_ABILITIES + (A.CANCEL_ORDER,)),
        denied_with={**market_support_off, **execution_denied},
    )


def test_admin_payload_with_market_support_enabled(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    both = frozenset(MarketSupportCapability)
    one = frozenset({MarketSupportCapability.MARKET_SUPPORT_LISTINGS})
    execution_denied = {a: R.ADMIN_CANNOT_EXECUTE for a in EXECUTION_GATED}
    retired = {a: R.LEGACY_ADMIN_MUTATION_RETIRED for a in (A.AMEND_ORDER, A.CANCEL_ORDER)}
    base = {**execution_denied, **retired}

    assert verdicts("admin", both) == expected(
        allowed=set(ADMIN_ABILITIES + MARKET_SUPPORT_ABILITIES), denied_with=base
    )
    assert verdicts("admin", one) == expected(
        allowed=set(ADMIN_ABILITIES + (A.MARKET_SUPPORT_ACCESS,)),
        denied_with={**base, A.MARKET_SUPPORT_ASSISTED_ORDERS: R.MARKET_SUPPORT_CAPABILITY_REQUIRED},
    )
    none = verdicts("admin", frozenset())
    assert none[A.MARKET_SUPPORT_ACCESS] == R.MARKET_SUPPORT_CAPABILITY_REQUIRED
    assert none[A.MARKET_SUPPORT_ASSISTED_ORDERS] == R.MARKET_SUPPORT_CAPABILITY_REQUIRED


def test_non_admin_never_gets_market_support_even_when_enabled(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    result = verdicts("buyer", frozenset(MarketSupportCapability))
    assert result[A.MARKET_SUPPORT_ACCESS] == R.ADMIN_ROLE_REQUIRED
    assert result[A.MARKET_SUPPORT_ASSISTED_ORDERS] == R.ADMIN_ROLE_REQUIRED


@pytest.mark.parametrize(
    ("kind", "code"),
    [
        ("org_pending", R.ORGANIZATION_NOT_APPROVED),
        ("kyc_rejected", R.KYC_REJECTED),
        ("kyc_other_org", R.KYC_ORGANIZATION_MISMATCH),
        ("email_unverified", R.EMAIL_NOT_VERIFIED),
        ("password_change", R.PASSWORD_CHANGE_REQUIRED),
        ("role_unassigned", R.ROLE_NOT_ASSIGNED),
        ("no_organization", R.ORGANIZATION_REQUIRED),
    ],
)
def test_blocked_users_lose_every_execution_ability_with_the_blocking_reason(kind, code):
    result = verdicts(kind)
    for ability in EXECUTION_GATED:
        assert result[ability] == code, ability
    # Cancelling is an owner-cleanup path that never required admission.
    assert result[A.CANCEL_ORDER] is None


def test_unknown_provenance_buyer_may_place_orders_but_not_execute_against_anyone():
    result = verdicts("unknown_provenance")
    assert result[A.PLACE_BID_ORDER] is None
    assert result[A.CONFIRM_TRADE] is None
    assert result[A.HIT_ASK_ORDER] == R.ORGANIZATION_PROVENANCE_UNVERIFIED
    assert result[A.CREATE_RFQ] == R.ORGANIZATION_PROVENANCE_UNVERIFIED


def test_unknown_provenance_supplier_may_publish_but_not_quote_or_hit():
    result = verdicts("supplier_unknown_provenance")
    assert result[A.PLACE_ASK_ORDER] is None
    assert result[A.PUBLISH_INVENTORY] is None
    assert result[A.HIT_BID_ORDER] == R.ORGANIZATION_PROVENANCE_UNVERIFIED
    assert result[A.QUOTE_RFQ] == R.ORGANIZATION_PROVENANCE_UNVERIFIED


def test_role_gated_pages_follow_the_role_table_for_every_user_kind():
    for kind in PROFILE_KINDS:
        user, organization = make_profile(kind)
        abilities = by_action(user, organization)
        for ability in ADMIN_ABILITIES:
            assert abilities[ability].allowed == (user.role in allowed_roles(ability))


# ---------------------------------------------------------------------------
# HTTP route
# ---------------------------------------------------------------------------


@asynccontextmanager
async def client_for(user, organization, *routers, assignments=()):
    app = FastAPI()
    for router in routers:
        app.include_router(router, prefix="/api")

    async def override_user():
        return user

    async def override_db():
        yield FakeDb(organization, assignments)

    app.dependency_overrides[get_current_user] = override_user
    app.dependency_overrides[get_db] = override_db
    transport = ASGITransport(app=app, raise_app_exceptions=False)
    async with AsyncClient(transport=transport, base_url="https://test") as client:
        yield client


@pytest.mark.asyncio
async def test_endpoint_returns_the_documented_shape():
    user, organization = make_profile("buyer")
    async with client_for(user, organization, abilities_router) as client:
        response = await client.get("/api/auth/me/abilities")

    assert response.status_code == 200
    assert response.headers["cache-control"] == "no-store"
    body = response.json()
    assert set(body) == {"schema_version", "abilities"}
    assert body["schema_version"] == SCHEMA_VERSION
    assert [item["action"] for item in body["abilities"]] == [a.value for a in AbilityId]
    for item in body["abilities"]:
        assert set(item) == {"action", "allowed", "reason_code", "reason"}
    by_id = {item["action"]: item for item in body["abilities"]}
    assert by_id["place_bid_order"] == {
        "action": "place_bid_order",
        "allowed": True,
        "reason_code": None,
        "reason": None,
    }
    assert by_id["place_ask_order"]["allowed"] is False
    assert by_id["place_ask_order"]["reason_code"] == "SUPPLIER_ROLE_REQUIRED"
    assert by_id["place_ask_order"]["reason"] == REASON_MESSAGES[R.SUPPLIER_ROLE_REQUIRED]


@pytest.mark.asyncio
async def test_endpoint_loads_staff_capabilities_for_an_enabled_administrator(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    user, organization = make_profile("admin")
    assignments = [
        StaffCapabilityAssignment(
            user_id=user.id, capability=capability, revoked_at=None, expires_at=None
        )
        for capability in MarketSupportCapability
    ]
    async with client_for(user, organization, abilities_router, assignments=assignments) as client:
        body = (await client.get("/api/auth/me/abilities")).json()

    by_id = {item["action"]: item for item in body["abilities"]}
    assert by_id["market_support_access"]["allowed"] is True
    assert by_id["market_support_assisted_orders"]["allowed"] is True


def test_route_is_mounted_and_uses_the_shared_user_lock_path():
    from app import main

    routes = {
        (method, route.path)
        for route in main.app.routes
        for method in getattr(route, "methods", set())
    }
    assert ("GET", "/api/auth/me/abilities") in routes
    assert "/api/auth/me/abilities" in AUTHENTICATED_SHARED_LOCK_PATHS


# ---------------------------------------------------------------------------
# Agreement: execution eligibility (shared by the middleware and the payload)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_execution_dependency_and_abilities_agree(kind):
    user, organization = make_profile(kind)
    abilities = by_action(user, organization)

    outcome = await gate_outcome(
        require_execution_eligible_user(current_user=user, db=FakeDb(organization))
    )
    eligible = await execution_party_is_eligible(FakeDb(organization), user=user)

    assert (outcome == "passed") == eligible
    assert eligible == (execution_party_denial(user, organization) is None)
    # Confirm and deliver are gated by that dependency alone.
    assert abilities[A.CONFIRM_TRADE].allowed == eligible
    assert abilities[A.DELIVER_TRADE].allowed == eligible


@pytest.mark.asyncio
async def test_admin_gets_its_own_denial_reason_from_the_dependency_predicate():
    user, organization = make_profile("admin")
    with pytest.raises(HTTPException) as error:
        await require_execution_eligible_user(current_user=user, db=FakeDb(organization))
    assert error.value.detail == "Administrators cannot execute market actions"
    assert by_action(user, organization)[A.CONFIRM_TRADE].reason_code == R.ADMIN_CANNOT_EXECUTE


# ---------------------------------------------------------------------------
# Agreement: place BID / ASK through the real create_order endpoint
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
@pytest.mark.parametrize("side", [OrderSide.BID, OrderSide.ASK])
async def test_create_order_and_place_order_abilities_agree(kind, side, monkeypatch):
    def stop_after_access_checks(_window):
        raise PassedGate

    monkeypatch.setattr(orderbook_module, "is_tradable_availability_window", stop_after_access_checks)
    user, organization = make_profile(kind)
    order = SimpleNamespace(side=side, support_confirmation=None, availability_window="SPOT")

    outcome = await gate_outcome(
        orderbook_module.create_order(
            request=SimpleNamespace(headers={}),
            order_data=order,
            current_user=user,
            db=FakeDb(organization),
        )
    )

    ability = A.PLACE_BID_ORDER if side == OrderSide.BID else A.PLACE_ASK_ORDER
    assert by_action(user, organization)[ability].allowed == (outcome == "passed")


# ---------------------------------------------------------------------------
# Agreement: amend and cancel (legacy administrator path + admission)
# ---------------------------------------------------------------------------


def _amend_stub_db(organization):
    class Db(FakeDb):
        async def execute(self, statement, *_a, **_k):
            return FakeResult(organization)

    return Db(organization)


@pytest.mark.asyncio
@pytest.mark.parametrize("market_support_enabled", [False, True])
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_amend_order_endpoint_and_ability_agree(kind, market_support_enabled, monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", market_support_enabled)
    user, organization = make_profile(kind)
    update = SimpleNamespace(expires_at=None, model_dump=lambda **_kw: {})

    # The endpoint answers "Order not found" (404) once its access checks pass.
    outcome = await gate_outcome(
        orderbook_module.update_order(
            order_id=uuid4(),
            request=SimpleNamespace(headers={}),
            update_data=update,
            current_user=user,
            db=_amend_stub_db(organization),
        )
    )

    assert by_action(user, organization)[A.AMEND_ORDER].allowed == (outcome == "passed")


@pytest.mark.asyncio
@pytest.mark.parametrize("market_support_enabled", [False, True])
@pytest.mark.parametrize("kind", ["buyer", "supplier", "admin", "org_pending", "kyc_rejected"])
async def test_cancel_order_endpoint_and_ability_agree(kind, market_support_enabled, monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", market_support_enabled)
    user, organization = make_profile(kind)

    # The endpoint answers "Order not found" (404) once its access checks pass.
    outcome = await gate_outcome(
        orderbook_module.cancel_order(
            order_id=uuid4(),
            request=SimpleNamespace(headers={}, method="DELETE"),
            current_user=user,
            db=_amend_stub_db(organization),
            if_match=None,
            body=None,
        )
    )

    assert by_action(user, organization)[A.CANCEL_ORDER].allowed == (outcome == "passed")


def test_legacy_admin_predicate_is_shared_by_the_router_and_the_payload(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    assert legacy_admin_workspace_mutation_denial(UserRole.ADMIN, has_support_context=False) == (
        R.LEGACY_ADMIN_MUTATION_RETIRED
    )
    assert legacy_admin_workspace_mutation_denial(UserRole.ADMIN, has_support_context=True) is None
    assert legacy_admin_workspace_mutation_denial(UserRole.BUYER, has_support_context=False) is None
    with pytest.raises(HTTPException) as error:
        orderbook_module._reject_admin_legacy_workspace_mutation(
            SimpleNamespace(headers={}), SimpleNamespace(role=UserRole.ADMIN)
        )
    assert error.value.status_code == 410


# ---------------------------------------------------------------------------
# Agreement: hit an order (create_trade) and mark paid (pay_trade)
# ---------------------------------------------------------------------------


def _hit_stub_db(order):
    class Db:
        def __init__(self):
            self.calls = 0

        async def execute(self, *_a, **_k):
            self.calls += 1
            if self.calls == 1:  # target slice lookup: skip the slice lock
                return SimpleNamespace(one_or_none=lambda: None)
            if self.calls == 2:  # locked order
                return SimpleNamespace(unique=lambda: SimpleNamespace(scalar_one_or_none=lambda: order))
            return SimpleNamespace(scalar_one_or_none=lambda: 1)  # catalog check

        async def rollback(self):
            return None

    return Db()


@pytest.mark.asyncio
# A user without an organization is turned away (400) before the role gate.
@pytest.mark.parametrize("kind", [k for k in PROFILE_KINDS if k != "no_organization"])
@pytest.mark.parametrize("target_side", [OrderSide.ASK, OrderSide.BID])
async def test_create_trade_role_gate_and_hit_abilities_agree(kind, target_side, monkeypatch):
    user, organization = make_profile(kind)
    order = SimpleNamespace(
        id=uuid4(),
        product_id=uuid4(),
        delivery_point_id=uuid4(),
        availability_window="SPOT",
        # The user's own order: after the role gate the endpoint answers 400
        # "Cannot trade with your own order", which marks the gate as passed.
        organization_id=user.organization_id,
        owner_user_id=uuid4(),
        side=target_side,
        status=OrderBookStatus.OPEN,
        expires_at=None,
        provenance=OrganizationProvenance.REAL,
    )
    monkeypatch.setattr(trades_module, "order_terms_digest", lambda _order: "digest")
    monkeypatch.setattr(trades_module, "order_is_execution_qualified", lambda _order: True)
    monkeypatch.setattr(trades_module, "is_demo_market_organization", lambda _org_id: False)
    monkeypatch.setattr(
        trades_module,
        "lock_and_load_market_organizations",
        _async_return({}),
    )
    payload = SimpleNamespace(order_id=order.id, expected_terms_digest="digest")

    try:
        await trades_module.create_trade(
            payload=payload,
            request=SimpleNamespace(headers={}),
            db=_hit_stub_db(order),
            current_user=user,
        )
    except HTTPException as error:
        role_denied = error.status_code == 403
    else:  # pragma: no cover - the stub order always stops the endpoint
        role_denied = False

    # The route's admission dependency (agreement test above) is not run when
    # calling the function directly, so compare the role verdict only.
    ability = A.HIT_ASK_ORDER if target_side == OrderSide.ASK else A.HIT_BID_ORDER
    reason = by_action(user, organization)[ability].reason_code
    role_codes = {R.BUYER_ROLE_REQUIRED, R.SUPPLIER_ROLE_REQUIRED}
    if execution_party_denial(user, organization) is None:
        assert role_denied == (reason in role_codes)
    assert role_denied == (hit_order_role_denial(user.role, target_side) is not None)


def _async_return(value):
    async def stub(*_args, **_kwargs):
        return value

    return stub


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_pay_trade_role_gate_and_ability_agree(kind, monkeypatch):
    user, organization = make_profile(kind)
    trade = SimpleNamespace(
        id=uuid4(),
        status=trades_module.TradeStatus.DELIVERED,
        buyer_id=uuid4(),
        buyer_user_id=uuid4(),
        seller_id=user.organization_id,
        seller_user_id=user.id,
    )

    async def stop_after_role_gate(*_args, **_kwargs):
        raise PassedGate

    monkeypatch.setattr(trades_module, "prepare_command_attempt", _async_return(None))
    monkeypatch.setattr(trades_module, "require_execution_eligible_user", _async_return(user))
    monkeypatch.setattr(trades_module, "_lock_trade_market_rows", _async_return((trade, None)))
    monkeypatch.setattr(trades_module, "lock_and_load_market_organizations", _async_return({}))
    monkeypatch.setattr(trades_module, "_authorize_assisted_trade_actor", _async_return(False))
    monkeypatch.setattr(trades_module, "_revalidate_trade_parties", stop_after_role_gate)

    outcome = await gate_outcome(
        trades_module.pay_trade(
            trade_id=trade.id,
            request=SimpleNamespace(headers={}),
            db=FakeDb(organization),
            current_user=user,
        )
    )

    reason = by_action(user, organization)[A.PAY_TRADE].reason_code
    role_codes = {R.BUYER_ROLE_REQUIRED, R.SUPPLIER_ROLE_REQUIRED}
    assert (outcome == "denied") == (user.role != UserRole.SUPPLIER)
    if execution_party_denial(user, organization) is None:
        assert (outcome == "denied") == (reason in role_codes)


# ---------------------------------------------------------------------------
# Agreement: supplier inventory
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_manage_inventory_endpoint_and_ability_agree(kind, monkeypatch):
    user, organization = make_profile(kind)

    async def stop_after_role_gate(*_args, **_kwargs):
        raise PassedGate

    monkeypatch.setattr(inventory_module, "lock_and_load_market_organizations", stop_after_role_gate)
    outcome = await gate_outcome(
        inventory_module.add_inventory(
            item=SimpleNamespace(), db=FakeDb(organization), current_user=user
        )
    )

    reason = by_action(user, organization)[A.MANAGE_INVENTORY].reason_code
    if execution_party_denial(user, organization) is None:
        assert (outcome == "denied") == (reason == R.SUPPLIER_ROLE_REQUIRED)
    assert (outcome == "denied") == (user.role != UserRole.SUPPLIER)


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_publish_inventory_endpoint_and_ability_agree(kind):
    user, organization = make_profile(kind)

    class Db:
        def __init__(self):
            self.calls = 0

        async def execute(self, *_a, **_k):
            self.calls += 1
            if self.calls == 1:  # locked user
                return SimpleNamespace(scalar_one_or_none=lambda: user)
            if self.calls == 2:  # locked organization
                return SimpleNamespace(scalar_one_or_none=lambda: organization)
            raise PassedGate  # the inventory item lookup follows both gates

        async def rollback(self):
            return None

    outcome = await gate_outcome(
        inventory_module.publish_inventory_item(
            item_id=uuid4(),
            request=SimpleNamespace(headers={}),
            db=Db(),
            current_user=user,
        )
    )

    # Without the dependency, only the in-route role and re-admission gates run.
    ability = by_action(user, organization)[A.PUBLISH_INVENTORY]
    if organization is not None and user.organization_id is not None:
        assert (outcome == "passed") == ability.allowed


# ---------------------------------------------------------------------------
# Agreement: RFQ through the real router (admission dependency + role gate)
# ---------------------------------------------------------------------------


RFQ_BODY = {
    "product_id": str(uuid4()),
    "delivery_point_id": str(uuid4()),
    "quantity_mt": "500",
}
QUOTE_BODY = {"price_per_mt_usd": "650"}


@pytest.fixture(autouse=True)
def _reset_rate_limiter():
    from app.rate_limit import limiter

    limiter.reset()
    yield


async def _rfq_gate_status(user, organization, path, body):
    app_state_limiter = __import__("app.rate_limit", fromlist=["limiter"]).limiter
    async with client_for(user, organization, rfq_router) as client:
        client._transport.app.state.limiter = app_state_limiter  # slowapi reads it
        response = await client.post(path, json=body)
    return response.status_code


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_create_rfq_route_and_ability_agree(kind):
    user, organization = make_profile(kind)
    status = await _rfq_gate_status(user, organization, "/api/rfq", RFQ_BODY)

    ability = by_action(user, organization)[A.CREATE_RFQ]
    # Provenance is enforced later, under row locks; every earlier gate is a 403.
    denied_before_locks = (not ability.allowed) and (
        ability.reason_code != R.ORGANIZATION_PROVENANCE_UNVERIFIED
    )
    assert (status == 403) == denied_before_locks


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_quote_rfq_route_and_ability_agree(kind):
    user, organization = make_profile(kind)
    status = await _rfq_gate_status(user, organization, f"/api/rfq/{uuid4()}/quote", QUOTE_BODY)

    ability = by_action(user, organization)[A.QUOTE_RFQ]
    denied_before_locks = (not ability.allowed) and (
        ability.reason_code != R.ORGANIZATION_PROVENANCE_UNVERIFIED
    )
    assert (status == 403) == denied_before_locks


# ---------------------------------------------------------------------------
# Agreement: provenance predicates
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("provenance", list(OrganizationProvenance))
def test_participant_provenance_predicate_matches_the_assertion(provenance):
    organization = Organization(
        id=uuid4(), name="Acme", type=OrgType.SHIPPING_LINE, provenance=provenance
    )
    eligible = participant_provenance_eligible(organization)
    try:
        assert_market_participant_provenance(organization)
        asserted = True
    except HTTPException as error:
        assert error.status_code == 409
        asserted = False
    assert eligible == asserted


@pytest.mark.parametrize("left", list(OrganizationProvenance))
@pytest.mark.parametrize("right", list(OrganizationProvenance))
def test_a_party_that_may_not_execute_is_never_compatible_with_anyone(left, right):
    if execution_provenance_compatible(left, right):
        assert provenance_may_execute(left) and provenance_may_execute(right)
    if not provenance_may_execute(left):
        assert not execution_provenance_compatible(left, right)


def test_only_real_and_demo_provenance_may_ever_execute():
    assert {p for p in OrganizationProvenance if provenance_may_execute(p)} == {
        OrganizationProvenance.REAL,
        OrganizationProvenance.DEMO,
    }


# ---------------------------------------------------------------------------
# Agreement: role-gated pages (require_role) and role predicates
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
@pytest.mark.parametrize("ability", ADMIN_ABILITIES)
async def test_require_role_dependency_and_ability_agree(kind, ability):
    user, organization = make_profile(kind)
    dependency = require_role(*allowed_roles(ability))

    outcome = await gate_outcome(dependency(current_user=user))

    assert by_action(user, organization)[ability].allowed == (outcome == "passed")
    assert role_may(ability, user.role) == (outcome == "passed")


def test_inline_admin_routers_read_the_same_role_table():
    # Commission, KYC review and account management check roles inline; they all
    # call role_may() with the same ability the payload reports.
    for ability in (A.MANAGE_COMMISSIONS, A.REVIEW_KYC, A.MANAGE_ACCOUNTS):
        assert role_may(ability, UserRole.ADMIN)
        assert not role_may(ability, UserRole.BUYER)
        assert not role_may(ability, UserRole.SUPPLIER)
        assert not role_may(ability, None)


@pytest.mark.parametrize("role", [*UserRole, None])
def test_order_role_predicates(role):
    assert (place_order_role_denial(role, OrderSide.BID) is None) == (role == UserRole.BUYER)
    assert (place_order_role_denial(role, OrderSide.ASK) is None) == (role == UserRole.SUPPLIER)
    assert (hit_order_role_denial(role, OrderSide.ASK) is None) == (role == UserRole.BUYER)
    assert (hit_order_role_denial(role, OrderSide.BID) is None) == (role == UserRole.SUPPLIER)


# ---------------------------------------------------------------------------
# Agreement: Market Support staff gate and capabilities
# ---------------------------------------------------------------------------


def _assignment(user, capability, **overrides):
    values = {
        "user_id": user.id,
        "capability": capability,
        "revoked_at": None,
        "expires_at": None,
    }
    values.update(overrides)
    return StaffCapabilityAssignment(**values)


def test_capability_activity_predicate():
    admin, _ = make_profile("admin")
    now = datetime.now(UTC)
    cap = MarketSupportCapability.MARKET_SUPPORT_LISTINGS
    assert capability_assignment_is_active(_assignment(admin, cap), now)
    assert capability_assignment_is_active(
        _assignment(admin, cap, expires_at=now + timedelta(hours=1)), now
    )
    assert not capability_assignment_is_active(_assignment(admin, cap, revoked_at=now), now)
    assert not capability_assignment_is_active(
        _assignment(admin, cap, expires_at=now - timedelta(seconds=1)), now
    )
    assert not capability_assignment_is_active(None, now)


@pytest.mark.asyncio
@pytest.mark.parametrize("enabled", [False, True])
@pytest.mark.parametrize("kind", ["admin", "buyer", "supplier"])
async def test_require_market_support_admin_and_access_ability_agree(kind, enabled, monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", enabled)
    user, organization = make_profile(kind)

    # Disabled Market Support answers 404 (the surface is hidden), others 403.
    try:
        await market_support_module.require_market_support_admin(current_user=user)
        outcome = "passed"
    except HTTPException as error:
        assert error.status_code in (403, 404)
        outcome = "denied"

    all_caps = frozenset(MarketSupportCapability)
    ability = by_action(user, organization, all_caps)[A.MARKET_SUPPORT_ACCESS]
    assert ability.allowed == (outcome == "passed")


@pytest.mark.asyncio
async def test_staff_gate_hides_the_surface_when_disabled_and_forbids_other_roles(monkeypatch):
    admin, _ = make_profile("admin")
    buyer, _ = make_profile("buyer")
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", False)
    with pytest.raises(HTTPException) as disabled:
        await market_support_module.require_market_support_admin(current_user=admin)
    assert disabled.value.status_code == 404

    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    with pytest.raises(HTTPException) as forbidden:
        await market_support_module.require_market_support_admin(current_user=buyer)
    assert forbidden.value.status_code == 403
    assert await market_support_module.require_market_support_admin(current_user=admin) is admin


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "granted",
    [
        (),
        (MarketSupportCapability.MARKET_SUPPORT_LISTINGS,),
        (MarketSupportCapability.MARKET_SUPPORT_AUTHORIZATIONS,),
        tuple(MarketSupportCapability),
    ],
)
async def test_context_capability_requirement_and_assisted_orders_ability_agree(granted, monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    admin, organization = make_profile("admin")
    assignments = [_assignment(admin, capability) for capability in granted]

    class LockingDb(FakeDb):
        async def execute(self, statement, *_a, **_k):
            # _lock_capability selects one capability row; match it by value.
            wanted = _requested_capability(statement)
            rows = [row for row in assignments if wanted is None or row.capability == wanted]
            return SimpleNamespace(
                scalar_one_or_none=lambda: rows[0] if rows else None,
                scalars=lambda: SimpleNamespace(all=lambda: list(assignments)),
            )

    outcome = await gate_outcome(
        market_support_module._require_context_capabilities(LockingDb(organization), admin.id)
    )

    ability = by_action(admin, organization, frozenset(granted))[A.MARKET_SUPPORT_ASSISTED_ORDERS]
    assert ability.allowed == (outcome == "passed")
    if not ability.allowed:
        assert ability.reason_code == R.MARKET_SUPPORT_CAPABILITY_REQUIRED


def _requested_capability(statement):
    compiled = statement.compile()
    for capability in MarketSupportCapability:
        if capability in compiled.params.values():
            return capability
    return None
