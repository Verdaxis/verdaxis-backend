"""Role rules and the stable vocabulary for the user abilities payload.

Every function here is the single definition of a role rule. The endpoint that
enforces the rule and ``app.services.abilities`` (the server-driven hints for
the frontend) both call it, so the two cannot drift apart. The module is
deliberately free of database and web-framework imports.

The vocabulary is a public contract: ``AbilityId`` and ``ReasonCode`` values are
consumed by the frontend. Add new members freely; never rename or remove one.
See docs/user-abilities.md.
"""
from __future__ import annotations

import enum
from collections.abc import Iterable

from app.models.orderbook import OrderSide
from app.models.user import UserRole


class AbilityId(str, enum.Enum):
    """Stable snake_case identifiers of the actions the payload describes."""

    # Order book and trade lifecycle
    PLACE_BID_ORDER = "place_bid_order"
    PLACE_ASK_ORDER = "place_ask_order"
    AMEND_ORDER = "amend_order"
    CANCEL_ORDER = "cancel_order"
    HIT_ASK_ORDER = "hit_ask_order"
    HIT_BID_ORDER = "hit_bid_order"
    CONFIRM_TRADE = "confirm_trade"
    DELIVER_TRADE = "deliver_trade"
    PAY_TRADE = "pay_trade"
    # Supplier inventory
    MANAGE_INVENTORY = "manage_inventory"
    PUBLISH_INVENTORY = "publish_inventory"
    # Request for quote
    CREATE_RFQ = "create_rfq"
    QUOTE_RFQ = "quote_rfq"
    # Administrator pages and actions
    VIEW_ADMIN_ANALYTICS = "view_admin_analytics"
    VIEW_AUDIT_LOG = "view_audit_log"
    VIEW_USER_ACTIVITY = "view_user_activity"
    VIEW_FEEDBACK = "view_feedback"
    MANAGE_SUBSCRIPTIONS = "manage_subscriptions"
    MANAGE_ACCOUNTS = "manage_accounts"
    REVIEW_KYC = "review_kyc"
    MANAGE_COMMISSIONS = "manage_commissions"
    # Market support (staff-assisted order entry)
    MARKET_SUPPORT_ACCESS = "market_support_access"
    MARKET_SUPPORT_ASSISTED_ORDERS = "market_support_assisted_orders"


class ReasonCode(str, enum.Enum):
    """Stable machine-readable reasons an ability is denied."""

    ADMIN_CANNOT_EXECUTE = "ADMIN_CANNOT_EXECUTE"
    ROLE_NOT_ASSIGNED = "ROLE_NOT_ASSIGNED"
    ORGANIZATION_REQUIRED = "ORGANIZATION_REQUIRED"
    ACCOUNT_NOT_APPROVED = "ACCOUNT_NOT_APPROVED"
    EMAIL_NOT_VERIFIED = "EMAIL_NOT_VERIFIED"
    PASSWORD_CHANGE_REQUIRED = "PASSWORD_CHANGE_REQUIRED"
    KYC_REQUIRED = "KYC_REQUIRED"
    KYC_REJECTED = "KYC_REJECTED"
    KYC_ORGANIZATION_MISMATCH = "KYC_ORGANIZATION_MISMATCH"
    ORGANIZATION_NOT_APPROVED = "ORGANIZATION_NOT_APPROVED"
    BUYER_ROLE_REQUIRED = "BUYER_ROLE_REQUIRED"
    SUPPLIER_ROLE_REQUIRED = "SUPPLIER_ROLE_REQUIRED"
    ADMIN_ROLE_REQUIRED = "ADMIN_ROLE_REQUIRED"
    ORGANIZATION_PROVENANCE_UNVERIFIED = "ORGANIZATION_PROVENANCE_UNVERIFIED"
    LEGACY_ADMIN_MUTATION_RETIRED = "LEGACY_ADMIN_MUTATION_RETIRED"
    MARKET_SUPPORT_DISABLED = "MARKET_SUPPORT_DISABLED"
    MARKET_SUPPORT_CAPABILITY_REQUIRED = "MARKET_SUPPORT_CAPABILITY_REQUIRED"


# Roles that may use each page/action whose only gate is the caller's role.
# Routers build their ``require_role`` dependency from this table.
_ROLES_BY_ABILITY: dict[AbilityId, tuple[UserRole, ...]] = {
    AbilityId.VIEW_ADMIN_ANALYTICS: (UserRole.ADMIN,),
    AbilityId.VIEW_AUDIT_LOG: (UserRole.ADMIN,),
    AbilityId.VIEW_USER_ACTIVITY: (UserRole.ADMIN,),
    AbilityId.VIEW_FEEDBACK: (UserRole.ADMIN,),
    AbilityId.MANAGE_SUBSCRIPTIONS: (UserRole.ADMIN,),
    AbilityId.MANAGE_ACCOUNTS: (UserRole.ADMIN,),
    AbilityId.REVIEW_KYC: (UserRole.ADMIN,),
    AbilityId.MANAGE_COMMISSIONS: (UserRole.ADMIN,),
}

_REASON_FOR_REQUIRED_ROLE: dict[UserRole, ReasonCode] = {
    UserRole.BUYER: ReasonCode.BUYER_ROLE_REQUIRED,
    UserRole.SUPPLIER: ReasonCode.SUPPLIER_ROLE_REQUIRED,
    UserRole.ADMIN: ReasonCode.ADMIN_ROLE_REQUIRED,
}


def role_permitted(role: UserRole | None, allowed_roles: Iterable[UserRole]) -> bool:
    """Return true when ``role`` is one of ``allowed_roles``."""
    return role in tuple(allowed_roles)


def allowed_roles(ability: AbilityId) -> tuple[UserRole, ...]:
    """Roles that may use a role-gated ability (see ``_ROLES_BY_ABILITY``)."""
    return _ROLES_BY_ABILITY[ability]


def role_may(ability: AbilityId, role: UserRole | None) -> bool:
    """Return true when ``role`` may use the role-gated ``ability``."""
    return role_permitted(role, allowed_roles(ability))


def role_gate_denial(ability: AbilityId, role: UserRole | None) -> ReasonCode | None:
    """Return why ``role`` may not use a role-gated ability, or ``None``."""
    roles = allowed_roles(ability)
    if role_permitted(role, roles):
        return None
    return _REASON_FOR_REQUIRED_ROLE[roles[0]]


def _required_role_denial(role: UserRole | None, required: UserRole) -> ReasonCode | None:
    if role == required:
        return None
    return _REASON_FOR_REQUIRED_ROLE[required]


def place_order_role_denial(role: UserRole | None, side: OrderSide) -> ReasonCode | None:
    """A buyer places BID orders; a supplier places ASK orders."""
    required = UserRole.BUYER if side == OrderSide.BID else UserRole.SUPPLIER
    return _required_role_denial(role, required)


def hit_order_role_denial(role: UserRole | None, target_side: OrderSide) -> ReasonCode | None:
    """A buyer hits an ASK order; a supplier hits a BID order."""
    required = UserRole.BUYER if target_side == OrderSide.ASK else UserRole.SUPPLIER
    return _required_role_denial(role, required)


def mark_trade_paid_role_denial(role: UserRole | None) -> ReasonCode | None:
    """Only a supplier (the seller) marks a trade as paid."""
    return _required_role_denial(role, UserRole.SUPPLIER)


def inventory_role_denial(role: UserRole | None) -> ReasonCode | None:
    """Only a supplier manages or publishes inventory."""
    return _required_role_denial(role, UserRole.SUPPLIER)


def create_rfq_role_denial(role: UserRole | None) -> ReasonCode | None:
    """Only a buyer creates a request for quote."""
    return _required_role_denial(role, UserRole.BUYER)


def quote_rfq_role_denial(role: UserRole | None) -> ReasonCode | None:
    """Only a supplier submits a quote on a request for quote."""
    return _required_role_denial(role, UserRole.SUPPLIER)
