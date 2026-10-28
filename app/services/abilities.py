"""Server-driven abilities: what the current user may do, and why not.

Nothing here decides a rule. Each ability is computed by calling the same
predicate the enforcing endpoint calls (``action_policy``, ``execution_policy``,
``market_admission``, ``provenance``, ``request_party``, ``market_support``).
Abilities are UI hints: each endpoint stays authoritative, and per-subject rules
(order ownership, trade party, order state) are outside this flat payload.

Contract: docs/user-abilities.md.
"""
from __future__ import annotations

from dataclasses import dataclass

from app.services.action_policy import (
    AbilityId,
    ReasonCode,
    create_rfq_role_denial,
    hit_order_role_denial,
    inventory_role_denial,
    mark_trade_paid_role_denial,
    place_order_role_denial,
    quote_rfq_role_denial,
    role_gate_denial,
)
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from app.models.market_support import MarketSupportCapability
from app.models.orderbook import OrderSide
from app.models.user import Organization, User
from app.services.execution_policy import execution_party_denial
from app.services.market_admission import participant_provenance_eligible
from app.services.market_support import (
    ASSISTED_ORDER_CAPABILITIES,
    active_capabilities,
    market_support_staff_denial,
)
from app.services.provenance import (
    provenance_may_execute,
    snapshot_organization_provenance,
)
from app.services.request_party import legacy_admin_workspace_mutation_denial

SCHEMA_VERSION = 1

REASON_MESSAGES: dict[ReasonCode, str] = {
    ReasonCode.ADMIN_CANNOT_EXECUTE: "Administrator accounts cannot place or execute market actions.",
    ReasonCode.ROLE_NOT_ASSIGNED: "Your account has no trading role yet.",
    ReasonCode.ORGANIZATION_REQUIRED: "Your account must belong to an organization.",
    ReasonCode.ACCOUNT_NOT_APPROVED: "Your account has not been approved yet.",
    ReasonCode.EMAIL_NOT_VERIFIED: "Verify your email address to continue.",
    ReasonCode.PASSWORD_CHANGE_REQUIRED: "Change your password to continue.",
    ReasonCode.KYC_REJECTED: "Your identity verification was rejected.",
    ReasonCode.KYC_ORGANIZATION_MISMATCH: "Your identity verification belongs to a different organization.",
    ReasonCode.ORGANIZATION_NOT_APPROVED: "Your organization has not been approved yet.",
    ReasonCode.BUYER_ROLE_REQUIRED: "Only buyer accounts can do this.",
    ReasonCode.SUPPLIER_ROLE_REQUIRED: "Only supplier accounts can do this.",
    ReasonCode.ADMIN_ROLE_REQUIRED: "Only administrator accounts can do this.",
    ReasonCode.ORGANIZATION_PROVENANCE_UNVERIFIED: "Your organization is not yet verified for live market trading.",
    ReasonCode.LEGACY_ADMIN_MUTATION_RETIRED: "Administrators must use an active Market Support context for this.",
    ReasonCode.MARKET_SUPPORT_DISABLED: "Market Support is not enabled.",
    ReasonCode.MARKET_SUPPORT_CAPABILITY_REQUIRED: "You have not been granted the Market Support capability this needs.",
}


@dataclass(frozen=True)
class Ability:
    action: AbilityId
    allowed: bool
    reason_code: ReasonCode | None = None

    @property
    def reason(self) -> str | None:
        return None if self.reason_code is None else REASON_MESSAGES[self.reason_code]


def _first(*denials: ReasonCode | None) -> ReasonCode | None:
    """Return the first denial, mirroring the order an endpoint applies its checks."""
    return next((denial for denial in denials if denial is not None), None)


def build_abilities(
    user: User,
    organization: Organization | None,
    active_support_capabilities: frozenset[MarketSupportCapability] = frozenset(),
) -> list[Ability]:
    """Evaluate every ability for one user from already-loaded facts (no I/O)."""
    role = user.role
    execution = execution_party_denial(user, organization)
    own_provenance_denial = (
        None
        if provenance_may_execute(snapshot_organization_provenance(organization))
        else ReasonCode.ORGANIZATION_PROVENANCE_UNVERIFIED
    )
    # No support-context header on this request: the legacy admin path applies.
    legacy_admin = legacy_admin_workspace_mutation_denial(role, has_support_context=False)

    denials: dict[AbilityId, ReasonCode | None] = {
        AbilityId.PLACE_BID_ORDER: _first(execution, place_order_role_denial(role, OrderSide.BID)),
        AbilityId.PLACE_ASK_ORDER: _first(execution, place_order_role_denial(role, OrderSide.ASK)),
        AbilityId.AMEND_ORDER: _first(legacy_admin, execution),
        AbilityId.CANCEL_ORDER: legacy_admin,
        AbilityId.HIT_ASK_ORDER: _first(
            execution, hit_order_role_denial(role, OrderSide.ASK), own_provenance_denial
        ),
        AbilityId.HIT_BID_ORDER: _first(
            execution, hit_order_role_denial(role, OrderSide.BID), own_provenance_denial
        ),
        AbilityId.CONFIRM_TRADE: execution,
        AbilityId.DELIVER_TRADE: execution,
        AbilityId.PAY_TRADE: _first(execution, mark_trade_paid_role_denial(role)),
        AbilityId.MANAGE_INVENTORY: _first(execution, inventory_role_denial(role)),
        AbilityId.PUBLISH_INVENTORY: _first(execution, inventory_role_denial(role)),
        AbilityId.CREATE_RFQ: _first(
            execution,
            create_rfq_role_denial(role),
            None
            if organization is not None and participant_provenance_eligible(organization)
            else ReasonCode.ORGANIZATION_PROVENANCE_UNVERIFIED,
        ),
        AbilityId.QUOTE_RFQ: _first(execution, quote_rfq_role_denial(role), own_provenance_denial),
    }
    for ability in (
        AbilityId.VIEW_ADMIN_ANALYTICS,
        AbilityId.VIEW_AUDIT_LOG,
        AbilityId.VIEW_USER_ACTIVITY,
        AbilityId.VIEW_FEEDBACK,
        AbilityId.MANAGE_SUBSCRIPTIONS,
        AbilityId.MANAGE_ACCOUNTS,
        AbilityId.REVIEW_KYC,
        AbilityId.MANAGE_COMMISSIONS,
    ):
        denials[ability] = role_gate_denial(ability, role)

    staff = market_support_staff_denial(user)
    needs_any = None if active_support_capabilities else ReasonCode.MARKET_SUPPORT_CAPABILITY_REQUIRED
    needs_all = (
        None
        if all(capability in active_support_capabilities for capability in ASSISTED_ORDER_CAPABILITIES)
        else ReasonCode.MARKET_SUPPORT_CAPABILITY_REQUIRED
    )
    denials[AbilityId.MARKET_SUPPORT_ACCESS] = _first(staff, needs_any)
    denials[AbilityId.MARKET_SUPPORT_ASSISTED_ORDERS] = _first(staff, needs_all)

    return [
        Ability(action=ability, allowed=denials[ability] is None, reason_code=denials[ability])
        for ability in AbilityId
    ]


async def compute_abilities(db: AsyncSession, user: User) -> list[Ability]:
    """Load the facts the predicates need and evaluate every ability."""
    organization = None
    if user.organization_id is not None:
        organization = (
            await db.execute(select(Organization).where(Organization.id == user.organization_id))
        ).scalar_one_or_none()
    capabilities: frozenset[MarketSupportCapability] = frozenset()
    if market_support_staff_denial(user) is None:
        capabilities = frozenset(await active_capabilities(db, user.id))
    return build_abilities(user, organization, capabilities)
