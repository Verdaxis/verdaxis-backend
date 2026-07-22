from __future__ import annotations

from app.models.orderbook import OrderBookOrder, OrderSide
from app.models.user import Organization, User, UserRole, UserStatus
from sqlalchemy import select


async def execution_party_is_eligible(db, *, user: User, organization: Organization | None = None) -> bool:
    """Return true only for a concrete admitted user and approved tenant."""
    user_id = getattr(user, "id", None) if user else None
    user_org_id = getattr(user, "organization_id", None) if user else None
    if user_id is None or user_org_id is None:
        return False
    if getattr(user, "role", None) not in {UserRole.BUYER, UserRole.SUPPLIER}:
        return False
    if getattr(user, "status", None) != UserStatus.APPROVED or not getattr(user, "email_verified", False):
        return False
    if getattr(user, "must_change_password", False):
        return False
    # KYC remains advisory for pending/legacy users. An explicit human
    # rejection revokes execution, and any org-bound state is valid only for
    # the user's current organization.
    if getattr(user, "kyc_status", None) == "REJECTED":
        return False
    kyc_organization_id = getattr(user, "kyc_organization_id", None)
    if kyc_organization_id is not None and kyc_organization_id != user_org_id:
        return False
    if organization is None and hasattr(db, "execute"):
        result = await db.execute(select(Organization).where(Organization.id == user.organization_id))
        organization = result.scalar_one_or_none()
    organization_id = getattr(organization, "id", None) if organization else None
    return (
        organization_id is not None
        and organization_id == user_org_id
        and getattr(organization, "verification_status", None) == "APPROVED"
    )


def normalize_certification_scheme(value: str | None) -> str | None:
    if value is None:
        return None
    normalized = value.strip().upper()
    return normalized or None


def order_is_execution_qualified(order: OrderBookOrder) -> bool:
    if getattr(order, "off_spec", False):
        return False

    normalized_scheme = normalize_certification_scheme(getattr(order, "certification_scheme", None))

    if getattr(order, "side", None) == OrderSide.ASK:
        if normalized_scheme is None:
            return False
        if not bool(getattr(order, "certification_declared", False)):
            return False

    return True


def orders_execution_compatible(left: OrderBookOrder, right: OrderBookOrder) -> bool:
    if not order_is_execution_qualified(left) or not order_is_execution_qualified(right):
        return False

    bid_order = left if getattr(left, "side", None) == OrderSide.BID else right
    ask_order = right if bid_order is left else left

    ask_scheme = normalize_certification_scheme(ask_order.certification_scheme)
    if ask_scheme is None:
        return True

    bid_certifications = [
        normalized
        for normalized in (
            normalize_certification_scheme(value)
            for value in getattr(bid_order, "certifications", []) or []
        )
        if normalized is not None
    ]
    if bid_certifications:
        return ask_scheme in set(bid_certifications)

    bid_scheme = normalize_certification_scheme(bid_order.certification_scheme)
    if bid_scheme is None:
        return True

    return bid_scheme == ask_scheme
