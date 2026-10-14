from __future__ import annotations

from app.market_catalog import PRODUCTS_BY_ID, PRODUCTS_BY_CODE
from app.models.orderbook import OrderBookOrder, OrderCreationMethod, OrderSide
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from sqlalchemy import select
from app.services.kyc_execution import kyc_execution_eligibility, kyc_execution_clause
from fastapi import HTTPException
from app.services.lng_order import (
    is_lng_product, lng_terms_compatible, validate_lng_order_terms,
)
from app.services.fame_order import (
    fame_terms_compatible, is_fame_product, validate_fame_order_terms,
)


async def execution_party_is_eligible(db, *, user: User, organization: Organization | None = None) -> bool:
    """Require admission and organization-bound KYC for new execution."""
    return await market_party_is_admitted(db, user=user, organization=organization, check_legacy_kyc=False) and (
        await kyc_execution_eligibility(db, user)
    ).eligible


async def market_party_is_admitted(db, *, user: User, organization: Organization | None = None, check_legacy_kyc: bool = True) -> bool:
    """Retain the existing admission policy for indicative, non-executable work."""
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
    if check_legacy_kyc:
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


async def customer_organization_is_execution_eligible(db, organization: Organization) -> bool:
    """Assisted execution requires one admitted economic member, not admin KYC."""
    if organization.verification_status != "APPROVED":
        return False
    statement = select(User.id).where(
        User.organization_id == organization.id,
        User.role.in_((UserRole.BUYER, UserRole.SUPPLIER)),
        User.status == UserStatus.APPROVED,
        User.email_verified.is_(True),
        User.must_change_password.is_(False),
        kyc_execution_clause(),
    ).order_by(User.id).limit(1).with_for_update(read=True, skip_locked=True)
    # Market rows precede UUID-ordered SHARE member locks. Concurrent
    # executions may share authority, but KYC UPDATE writers conflict. Skip contended members:
    # callers can already hold organizations, and waiting here would invert
    # the KYC writer's user-before-organization lock order. A skipped member
    # cannot authorize execution; a selected member stays locked until commit.
    return (await db.execute(statement)).scalar_one_or_none() is not None


async def require_customer_organization_execution(db, organization: Organization) -> None:
    """Expose no member identity when the customer's organization is blocked."""
    if not await customer_organization_is_execution_eligible(db, organization):
        raise HTTPException(403, detail={
            "code": "KYC_REQUIRED",
            "message": "Complete KYC verification to trade.",
            "kyc_status": "PENDING",
            "grace_expires_at": None,
        })


async def order_owner_is_execution_eligible(
    db,
    *,
    order: OrderBookOrder,
    user: User | None,
    organization: Organization | None,
) -> bool:
    """Revalidate either a self-service principal or an assisted-order actor."""
    if order.creation_method != OrderCreationMethod.MARKET_SUPPORT:
        return await execution_party_is_eligible(db, user=user, organization=organization)
    return bool(
        user
        and organization
        and user.id == order.created_by_actor_user_id == order.owner_user_id
        and user.role == UserRole.ADMIN
        and user.status == UserStatus.APPROVED
        and organization.id == order.organization_id
        and organization.verification_status == "APPROVED"
        and organization.provenance == OrganizationProvenance.REAL
        and await customer_organization_is_execution_eligible(db, organization)
    )


def normalize_certification_scheme(value: str | None) -> str | None:
    if value is None:
        return None
    normalized = value.strip().upper()
    return normalized or None


def order_is_execution_qualified(order: OrderBookOrder) -> bool:
    spec = PRODUCTS_BY_ID.get(getattr(order, "product_id", None))
    if spec is None:
        spec = PRODUCTS_BY_CODE.get(getattr(order, "market_product", None))
    if spec is not None and spec.execution_mode != "ORDERBOOK":
        return False
    if getattr(order, "off_spec", False):
        return False
    if is_lng_product(getattr(order, "product_id", None)):
        if getattr(order, "delivery_point_id", None) not in spec.available_delivery_point_ids:
            return False
        if getattr(order, "side", None) == OrderSide.ASK and not getattr(order, "msds_available", False):
            return False
        try:
            validate_lng_order_terms(order.product_id, order.side, getattr(order, "lng_terms", None))
        except HTTPException:
            return False
    if is_fame_product(getattr(order, "product_id", None)):
        if getattr(order, "side", None) == OrderSide.ASK and not getattr(order, "msds_available", False):
            return False
        try:
            validate_fame_order_terms(
                order.product_id, order.side, getattr(order, "fame_terms", None),
                getattr(order, "availability_window", "SPOT"),
            )
        except HTTPException:
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

    if is_fame_product(getattr(bid_order, "product_id", None)) or is_fame_product(getattr(ask_order, "product_id", None)):
        if bid_order.product_id != ask_order.product_id:
            return False
        # B100's structured scheme and specification are the canonical terms;
        # legacy display aliases must not introduce a second matching rule.
        return fame_terms_compatible(getattr(bid_order, "fame_terms", None), getattr(ask_order, "fame_terms", None))

    if is_lng_product(getattr(bid_order, "product_id", None)) or is_lng_product(getattr(ask_order, "product_id", None)):
        return bid_order.product_id == ask_order.product_id and lng_terms_compatible(
            getattr(bid_order, "lng_terms", None), getattr(ask_order, "lng_terms", None)
        )

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
