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
from app.services.abilities import (
    POLICY_BY_ABILITY,
    REASON_MESSAGES,
    SCHEMA_VERSION,
    build_abilities,
)
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
from app.middleware.execution import (
    require_market_admitted_user,
    require_market_cleanup_user,
)
from app.services.execution_policy import (
    MarketPartyPolicy,
    market_party_denial,
    market_party_is_eligible,
)
from app.services.market_admission import (
    MarketActorOwnership,
    lock_and_load_market_organizations,
)
from app.services.kyc_execution import (
    advisory_kyc_allows,
    evaluate_kyc_execution,
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
# Every ability whose endpoint applies a market-party policy.
PARTY_GATED = tuple(POLICY_BY_ABILITY)
EXECUTION_GATED = PARTY_GATED
GATE_SWITCH = "app.config.settings.KYC_EXECUTION_GATE_ENABLED"


@pytest.fixture(autouse=True, params=[True, False], ids=["gate-on", "gate-off"])
def kyc_gate(request, monkeypatch):
    """Run every test with the KYC execution gate on and off."""
    monkeypatch.setattr(GATE_SWITCH, request.param)
    return request.param


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
        kyc_organization_id=organization_id,
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
    elif kind in {"kyc_pending", "kyc_grace", "kyc_grace_expired"}:
        user.kyc_status = "PENDING"
        user.kyc_organization_id = None
        if kind == "kyc_grace":
            GRACE_BY_USER[user.id] = datetime.now(UTC) + timedelta(days=3)
        elif kind == "kyc_grace_expired":
            GRACE_BY_USER[user.id] = datetime.now(UTC) - timedelta(days=1)
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


GRACE_BY_USER: dict = {}  # user id -> kyc_execution_grace.expires_at

PROFILE_KINDS = (
    "buyer",
    "supplier",
    "admin",
    "org_pending",
    "kyc_rejected",
    "kyc_other_org",
    "kyc_pending",
    "kyc_grace",
    "kyc_grace_expired",
    "unknown_provenance",
    "supplier_unknown_provenance",
    "email_unverified",
    "password_change",
    "role_unassigned",
    "no_organization",
)


async def party_denials(user, organization):
    """The market-party denial for each policy, from the real predicate."""
    return {
        policy: await market_party_denial(
            FakeDb(organization), user=user, organization=organization, policy=policy
        )
        for policy in MarketPartyPolicy
    }


async def abilities_for(user, organization, capabilities=frozenset()):
    return build_abilities(user, organization, await party_denials(user, organization), capabilities)


async def by_action(user, organization, capabilities=frozenset()):
    return {a.action: a for a in await abilities_for(user, organization, capabilities)}


async def party_allows(user, organization, ability):
    """Does the policy this ability's endpoint applies admit the user?"""
    denial = await market_party_denial(
        FakeDb(organization),
        user=user,
        organization=organization,
        policy=POLICY_BY_ABILITY[ability],
    )
    return denial is None


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


def grace_result(statement):
    """Answer the KYC grace lookup, or return None for any other statement."""
    if "kyc_execution_grace" not in str(statement):
        return None
    expires_at = next(
        (
            GRACE_BY_USER[value]
            for value in statement.compile().params.values()
            if value in GRACE_BY_USER
        ),
        None,
    )
    return SimpleNamespace(scalar_one_or_none=lambda: expires_at)


class FakeDb:
    """Answers every select with the user's organization (or no rows)."""

    def __init__(self, organization, assignments=()):
        self._organization = organization
        self._assignments = assignments

    async def execute(self, statement, *_args, **_kwargs):
        return grace_result(statement) or FakeResult(self._organization, self._assignments)

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


async def test_every_ability_is_reported_once_in_declaration_order():
    user, organization = make_profile("buyer")
    abilities = await abilities_for(user, organization)
    assert [a.action for a in abilities] == list(AbilityId)
    assert len({a.action for a in abilities}) == len(abilities)


def test_ids_and_codes_are_snake_case_and_upper_case_and_every_code_has_a_message():
    assert all(a.value == a.value.lower() and " " not in a.value for a in AbilityId)
    assert all(c.value == c.name and c.value == c.value.upper() for c in ReasonCode)
    assert set(REASON_MESSAGES) == set(ReasonCode)


async def test_allowed_abilities_carry_no_reason_and_denied_abilities_always_do():
    for kind in PROFILE_KINDS:
        user, organization = make_profile(kind)
        for ability in await abilities_for(user, organization):
            if ability.allowed:
                assert ability.reason_code is None and ability.reason is None
            else:
                assert ability.reason_code is not None
                assert ability.reason == REASON_MESSAGES[ability.reason_code]


async def verdicts(kind, capabilities=frozenset()):
    user, organization = make_profile(kind)
    abilities = await by_action(user, organization, capabilities)
    return {action: ability.reason_code for action, ability in abilities.items()}


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


async def test_buyer_payload():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    admin_denied = {a: R.ADMIN_ROLE_REQUIRED for a in ADMIN_ABILITIES}
    supplier_denied = {a: R.SUPPLIER_ROLE_REQUIRED for a in SUPPLIER_ONLY}
    assert await verdicts("buyer") == expected(
        allowed=set(BUYER_ONLY + ANY_PARTICIPANT + (A.CANCEL_ORDER,)),
        denied_with={**market_support_off, **admin_denied, **supplier_denied},
    )


async def test_supplier_payload():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    admin_denied = {a: R.ADMIN_ROLE_REQUIRED for a in ADMIN_ABILITIES}
    buyer_denied = {a: R.BUYER_ROLE_REQUIRED for a in BUYER_ONLY}
    assert await verdicts("supplier") == expected(
        allowed=set(SUPPLIER_ONLY + ANY_PARTICIPANT + (A.CANCEL_ORDER,)),
        denied_with={**market_support_off, **admin_denied, **buyer_denied},
    )


async def test_admin_payload_with_market_support_disabled():
    market_support_off = {a: R.MARKET_SUPPORT_DISABLED for a in MARKET_SUPPORT_ABILITIES}
    execution_denied = {a: R.ADMIN_CANNOT_EXECUTE for a in EXECUTION_GATED}
    assert await verdicts("admin") == expected(
        allowed=set(ADMIN_ABILITIES),
        denied_with={**market_support_off, **execution_denied},
    )


async def test_admin_payload_with_market_support_enabled(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    both = frozenset(MarketSupportCapability)
    one = frozenset({MarketSupportCapability.MARKET_SUPPORT_LISTINGS})
    execution_denied = {a: R.ADMIN_CANNOT_EXECUTE for a in EXECUTION_GATED}
    retired = {a: R.LEGACY_ADMIN_MUTATION_RETIRED for a in (A.AMEND_ORDER, A.CANCEL_ORDER)}
    base = {**execution_denied, **retired}

    assert await verdicts("admin", both) == expected(
        allowed=set(ADMIN_ABILITIES + MARKET_SUPPORT_ABILITIES), denied_with=base
    )
    assert await verdicts("admin", one) == expected(
        allowed=set(ADMIN_ABILITIES + (A.MARKET_SUPPORT_ACCESS,)),
        denied_with={**base, A.MARKET_SUPPORT_ASSISTED_ORDERS: R.MARKET_SUPPORT_CAPABILITY_REQUIRED},
    )
    none = await verdicts("admin", frozenset())
    assert none[A.MARKET_SUPPORT_ACCESS] == R.MARKET_SUPPORT_CAPABILITY_REQUIRED
    assert none[A.MARKET_SUPPORT_ASSISTED_ORDERS] == R.MARKET_SUPPORT_CAPABILITY_REQUIRED


async def test_non_admin_never_gets_market_support_even_when_enabled(monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", True)
    result = await verdicts("buyer", frozenset(MarketSupportCapability))
    assert result[A.MARKET_SUPPORT_ACCESS] == R.ADMIN_ROLE_REQUIRED
    assert result[A.MARKET_SUPPORT_ASSISTED_ORDERS] == R.ADMIN_ROLE_REQUIRED


@pytest.mark.parametrize(
    ("kind", "code"),
    [
        ("org_pending", R.ORGANIZATION_NOT_APPROVED),
        ("email_unverified", R.EMAIL_NOT_VERIFIED),
        ("password_change", R.PASSWORD_CHANGE_REQUIRED),
        ("role_unassigned", R.ROLE_NOT_ASSIGNED),
        ("no_organization", R.ORGANIZATION_REQUIRED),
    ],
)
async def test_blocked_accounts_lose_every_market_ability_under_every_policy(kind, code):
    result = await verdicts(kind)
    for ability in PARTY_GATED:
        assert result[ability] == code, ability


def kyc_expectation(kind: str, gate_enabled: bool) -> dict[MarketPartyPolicy, R | None]:
    """What the KYC rule does to each policy, written out from the gate docs."""
    cleanup = None  # KYC never blocks settling or withdrawing a commitment
    if kind == "kyc_rejected":  # a human rejection blocks execution on every path
        return {
            MarketPartyPolicy.EXECUTION: R.KYC_REJECTED,
            MarketPartyPolicy.INDICATIVE: R.KYC_REJECTED,
            MarketPartyPolicy.CLEANUP: cleanup,
        }
    if kind == "kyc_other_org":  # approved for another organization
        return {
            MarketPartyPolicy.EXECUTION: R.KYC_REQUIRED,
            MarketPartyPolicy.INDICATIVE: R.KYC_ORGANIZATION_MISMATCH,
            MarketPartyPolicy.CLEANUP: cleanup,
        }
    if kind in {"kyc_pending", "kyc_grace_expired"}:
        # Not approved and no valid grace: blocked only while the gate is on.
        return {
            MarketPartyPolicy.EXECUTION: R.KYC_REQUIRED if gate_enabled else None,
            MarketPartyPolicy.INDICATIVE: None,
            MarketPartyPolicy.CLEANUP: cleanup,
        }
    # buyer (approved for its organization) and kyc_grace (valid grace)
    return {policy: None for policy in MarketPartyPolicy}


@pytest.mark.parametrize(
    "kind",
    ["buyer", "kyc_rejected", "kyc_other_org", "kyc_pending", "kyc_grace", "kyc_grace_expired"],
)
async def test_kyc_gate_outcomes_per_policy_and_ability(kind, kyc_gate):
    user, organization = make_profile(kind)
    expected_by_policy = kyc_expectation(kind, kyc_gate)

    assert await party_denials(user, organization) == expected_by_policy

    # Buyer-side abilities the buyer holds role-wise: reason == the policy's code.
    buyer_abilities = (A.PLACE_BID_ORDER, A.AMEND_ORDER, A.CANCEL_ORDER, A.HIT_ASK_ORDER)
    buyer_abilities += (A.CONFIRM_TRADE, A.DELIVER_TRADE, A.CREATE_RFQ)
    result = await verdicts(kind)
    for ability in buyer_abilities:
        assert result[ability] == expected_by_policy[POLICY_BY_ABILITY[ability]], ability


async def test_gate_switch_changes_only_the_execution_policy(kyc_gate):
    pending, organization = make_profile("kyc_pending")
    denials = await party_denials(pending, organization)
    assert (denials[MarketPartyPolicy.EXECUTION] is None) == (not kyc_gate)
    assert denials[MarketPartyPolicy.INDICATIVE] is None
    assert denials[MarketPartyPolicy.CLEANUP] is None


async def test_grace_admits_execution_only_while_valid_and_the_gate_is_on(kyc_gate):
    valid, organization = make_profile("kyc_grace")
    expired, _ = make_profile("kyc_grace_expired")
    assert (await evaluate_kyc_execution(FakeDb(organization), valid)).eligible
    # Past its deadline a grace row admits nothing; with the gate off the
    # advisory rule admits the user regardless of grace.
    assert (await evaluate_kyc_execution(FakeDb(organization), expired)).eligible == (not kyc_gate)


async def test_unknown_provenance_buyer_may_place_orders_but_not_execute_against_anyone():
    result = await verdicts("unknown_provenance")
    assert result[A.PLACE_BID_ORDER] is None
    assert result[A.CONFIRM_TRADE] is None
    assert result[A.HIT_ASK_ORDER] == R.ORGANIZATION_PROVENANCE_UNVERIFIED
    assert result[A.CREATE_RFQ] == R.ORGANIZATION_PROVENANCE_UNVERIFIED


async def test_unknown_provenance_supplier_may_publish_but_not_quote_or_hit():
    result = await verdicts("supplier_unknown_provenance")
    assert result[A.PLACE_ASK_ORDER] is None
    assert result[A.PUBLISH_INVENTORY] is None
    assert result[A.HIT_BID_ORDER] == R.ORGANIZATION_PROVENANCE_UNVERIFIED
    assert result[A.QUOTE_RFQ] == R.ORGANIZATION_PROVENANCE_UNVERIFIED


async def test_role_gated_pages_follow_the_role_table_for_every_user_kind():
    for kind in PROFILE_KINDS:
        user, organization = make_profile(kind)
        abilities = await by_action(user, organization)
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


DEPENDENCIES = (
    (require_execution_eligible_user, MarketPartyPolicy.EXECUTION),
    (require_market_cleanup_user, MarketPartyPolicy.CLEANUP),
    (require_market_admitted_user, MarketPartyPolicy.INDICATIVE),
)


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
@pytest.mark.parametrize(("dependency", "policy"), DEPENDENCIES, ids=["execution", "cleanup", "indicative"])
async def test_market_party_dependency_predicate_and_abilities_agree(kind, dependency, policy):
    user, organization = make_profile(kind)
    abilities = await by_action(user, organization)

    outcome = await gate_outcome(dependency(current_user=user, db=FakeDb(organization)))
    eligible = await market_party_is_eligible(
        FakeDb(organization), user=user, organization=organization, policy=policy
    )
    denial = await market_party_denial(
        FakeDb(organization), user=user, organization=organization, policy=policy
    )

    assert (outcome == "passed") == eligible == (denial is None)
    # Every ability whose endpoint applies this policy is denied with the
    # predicate's own reason whenever the predicate denies. (When it admits,
    # role or provenance gates may still deny; those have their own tests.)
    gated = [ability for ability, used in POLICY_BY_ABILITY.items() if used == policy]
    assert gated
    for ability in gated:
        if not eligible:
            assert abilities[ability].reason_code == denial, ability


def _policies_an_endpoint_applies(function) -> set[MarketPartyPolicy]:
    """Which market-party policy the endpoint applies, read from the code.

    A gate is either a ``Depends(...)`` default in the signature or a call in
    the body; cancel (no admission gate) applies CLEANUP when it re-locks the
    actor's organization.
    """
    import inspect

    gate_policy = {
        require_execution_eligible_user: MarketPartyPolicy.EXECUTION,
        require_market_cleanup_user: MarketPartyPolicy.CLEANUP,
        require_market_admitted_user: MarketPartyPolicy.INDICATIVE,
    }
    found = set()
    import typing

    unwrapped = inspect.unwrap(function)
    annotations = typing.get_type_hints(unwrapped, include_extras=True)
    for parameter in inspect.signature(unwrapped).parameters.values():
        candidates = [parameter.default, *getattr(annotations.get(parameter.name), "__metadata__", ())]
        for candidate in candidates:
            dependency = getattr(candidate, "dependency", None)
            if dependency in gate_policy:
                found.add(gate_policy[dependency])
    source = inspect.getsource(inspect.unwrap(function))
    for gate, policy in gate_policy.items():
        if f"await {gate.__name__}(" in source:
            found.add(policy)
    if "actor_policy=MarketPartyPolicy.CLEANUP" in source:
        found.add(MarketPartyPolicy.CLEANUP)
    return found


def test_policy_table_matches_the_policy_each_real_endpoint_applies():
    from app.routers import rfq as rfq_module

    endpoints = {
        A.PLACE_BID_ORDER: orderbook_module.create_order,
        A.PLACE_ASK_ORDER: orderbook_module.create_order,
        A.AMEND_ORDER: orderbook_module.update_order,
        A.CANCEL_ORDER: orderbook_module.cancel_order,
        A.HIT_ASK_ORDER: trades_module.create_trade,
        A.HIT_BID_ORDER: trades_module.create_trade,
        A.CONFIRM_TRADE: trades_module.confirm_trade,
        A.DELIVER_TRADE: trades_module.deliver_trade,
        A.PAY_TRADE: trades_module.pay_trade,
        A.MANAGE_INVENTORY: inventory_module.add_inventory,
        A.PUBLISH_INVENTORY: inventory_module.publish_inventory_item,
        A.CREATE_RFQ: rfq_module.create_rfq,
        A.QUOTE_RFQ: rfq_module.submit_quote,
    }
    assert set(endpoints) == set(POLICY_BY_ABILITY)
    for ability, endpoint in endpoints.items():
        applied = _policies_an_endpoint_applies(endpoint)
        assert POLICY_BY_ABILITY[ability] in applied, (ability, applied)
    # Inventory add, update and delete share one gate.
    for function in (inventory_module.update_inventory, inventory_module.delete_inventory):
        assert _policies_an_endpoint_applies(function) == {MarketPartyPolicy.INDICATIVE}


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
@pytest.mark.parametrize("policy", list(MarketPartyPolicy))
async def test_lock_and_load_revalidation_agrees_with_the_policy_predicate(kind, policy):
    """The in-transaction recheck (deliver, pay, cancel, RFQ...) is the same predicate."""
    user, organization = make_profile(kind)
    if organization is None or user.organization_id is None:
        return  # the router rejects an organization-less user before locking

    class LockDb:
        async def execute(self, statement, *_a, **_k):
            grace = grace_result(statement)
            if grace is not None:
                return grace
            rows = [user] if "FROM users" in str(statement) else [organization]
            return SimpleNamespace(scalars=lambda: iter(rows))

    try:
        await lock_and_load_market_organizations(
            LockDb(),
            [organization.id],
            actor_ownerships=(MarketActorOwnership(user.id, organization.id),),
            actor_policy=policy,
        )
        outcome = "passed"
    except HTTPException as error:
        assert error.status_code == 409  # the in-transaction recheck is a conflict
        outcome = "denied"
    denial = await market_party_denial(
        FakeDb(organization), user=user, organization=organization, policy=policy
    )
    assert (outcome == "passed") == (denial is None)


@pytest.mark.asyncio
async def test_admin_gets_its_own_denial_reason_from_the_dependency_predicate():
    user, organization = make_profile("admin")
    for dependency in (
        require_execution_eligible_user,
        require_market_cleanup_user,
        require_market_admitted_user,
    ):
        with pytest.raises(HTTPException) as error:
            await dependency(current_user=user, db=FakeDb(organization))
        assert error.value.detail == "Administrators cannot execute market actions"
    assert (await by_action(user, organization))[A.CONFIRM_TRADE].reason_code == R.ADMIN_CANNOT_EXECUTE


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["kyc_pending", "kyc_rejected", "kyc_grace_expired"])
async def test_execution_gate_http_error_and_reason_code_share_the_kyc_required_code(kind, kyc_gate):
    user, organization = make_profile(kind)
    denial = await market_party_denial(
        FakeDb(organization), user=user, organization=organization,
        policy=MarketPartyPolicy.EXECUTION,
    )
    try:
        await require_execution_eligible_user(current_user=user, db=FakeDb(organization))
    except HTTPException as error:
        assert error.status_code == 403
        assert error.detail["code"] == "KYC_REQUIRED"
        assert denial in (R.KYC_REQUIRED, R.KYC_REJECTED)
    else:
        assert denial is None


# ---------------------------------------------------------------------------
# Agreement: place BID / ASK through the real create_order endpoint
# ---------------------------------------------------------------------------


# A user without an organization is answered 400 "User must belong to an
# organization" before the gate runs; the payload reports ORGANIZATION_REQUIRED.
@pytest.mark.asyncio
@pytest.mark.parametrize("kind", [k for k in PROFILE_KINDS if k != "no_organization"])
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
    assert (await by_action(user, organization))[ability].allowed == (outcome == "passed")


# ---------------------------------------------------------------------------
# Agreement: amend and cancel (legacy administrator path + admission)
# ---------------------------------------------------------------------------


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
            db=FakeDb(organization),
        )
    )

    assert (await by_action(user, organization))[A.AMEND_ORDER].allowed == (outcome == "passed")


@pytest.mark.asyncio
@pytest.mark.parametrize("market_support_enabled", [False, True])
@pytest.mark.parametrize("kind", ["buyer", "supplier", "admin", "org_pending", "kyc_rejected", "kyc_pending"])
async def test_cancel_order_endpoint_and_ability_agree(kind, market_support_enabled, monkeypatch):
    monkeypatch.setattr("app.config.settings.MARKET_SUPPORT_ENABLED", market_support_enabled)
    user, organization = make_profile(kind)

    # Cancel has no admission dependency: the only gate before the order lookup
    # is the retired legacy administrator path (410). The CLEANUP party check
    # runs later under row locks and is covered by the lock-and-load test.
    outcome = await gate_outcome(
        orderbook_module.cancel_order(
            order_id=uuid4(),
            request=SimpleNamespace(headers={}, method="DELETE"),
            current_user=user,
            db=FakeDb(organization),
            if_match=None,
            body=None,
        )
    )

    ability = (await by_action(user, organization))[A.CANCEL_ORDER]
    assert (outcome == "denied") == (ability.reason_code == R.LEGACY_ADMIN_MUTATION_RETIRED)


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
    # The in-route EXECUTION gate has its own agreement test; stub it here so
    # this test isolates the role gate that follows it.
    monkeypatch.setattr(trades_module, "require_execution_eligible_user", _async_return(user))
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

    # The EXECUTION gate is stubbed (agreement test above): compare the role verdict only.
    ability = A.HIT_ASK_ORDER if target_side == OrderSide.ASK else A.HIT_BID_ORDER
    reason = (await by_action(user, organization))[ability].reason_code
    role_codes = {R.BUYER_ROLE_REQUIRED, R.SUPPLIER_ROLE_REQUIRED}
    if await party_allows(user, organization, ability):
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
    # Paying settles a delivered commitment: the CLEANUP gate, never KYC.
    monkeypatch.setattr(trades_module, "require_market_cleanup_user", _async_return(user))
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

    reason = (await by_action(user, organization))[A.PAY_TRADE].reason_code
    role_codes = {R.BUYER_ROLE_REQUIRED, R.SUPPLIER_ROLE_REQUIRED}
    assert (outcome == "denied") == (user.role != UserRole.SUPPLIER)
    if await party_allows(user, organization, A.PAY_TRADE):
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

    reason = (await by_action(user, organization))[A.MANAGE_INVENTORY].reason_code
    if await party_allows(user, organization, A.MANAGE_INVENTORY):
        assert (outcome == "denied") == (reason == R.SUPPLIER_ROLE_REQUIRED)
    assert (outcome == "denied") == (user.role != UserRole.SUPPLIER)


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", PROFILE_KINDS)
async def test_publish_inventory_endpoint_and_ability_agree(kind):
    user, organization = make_profile(kind)

    class Db:
        def __init__(self):
            self.calls = 0

        async def execute(self, statement, *_a, **_k):
            grace = grace_result(statement)
            if grace is not None:  # the KYC rule's grace lookup is not a gate step
                return grace
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
    ability = (await by_action(user, organization))[A.PUBLISH_INVENTORY]
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

    ability = (await by_action(user, organization))[A.CREATE_RFQ]
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

    ability = (await by_action(user, organization))[A.QUOTE_RFQ]
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

    assert (await by_action(user, organization))[ability].allowed == (outcome == "passed")
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
    ability = (await by_action(user, organization, all_caps))[A.MARKET_SUPPORT_ACCESS]
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

    ability = (await by_action(admin, organization, frozenset(granted)))[A.MARKET_SUPPORT_ASSISTED_ORDERS]
    assert ability.allowed == (outcome == "passed")
    if not ability.allowed:
        assert ability.reason_code == R.MARKET_SUPPORT_CAPABILITY_REQUIRED


def _requested_capability(statement):
    compiled = statement.compile()
    for capability in MarketSupportCapability:
        if capability in compiled.params.values():
            return capability
    return None
