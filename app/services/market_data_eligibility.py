"""SQL predicates for formal market evidence.

Only approved organizations with REAL immutable provenance are eligible for
formal evidence. UNKNOWN remains quarantined. No name/domain heuristics.
"""
from __future__ import annotations

from sqlalchemy import and_, case, func, or_, select

from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
from app.models.orderbook import OrderCreationMethod
from app.models.user import (
    Organization,
    OrganizationProvenance,
    User,
    UserRole,
    UserStatus,
)


def public_order_owner_admission_clause(order):
    """Exclude orders whose recorded owner can no longer execute.

    Query-surface mirror of the fill-time owner policies: an order whose
    concrete owner was KYC- or admin-rejected (or is unverified, forced into
    a password change, moved to another organization, or whose organization
    lost approval) is permanently inert at fill time, so it must not appear
    as public liquidity nor weigh in any benchmark. Assisted orders use the
    narrower ``order_owner_is_execution_eligible`` ADMIN actor policy.
    Rows with no recorded owner (synthetic DEMO liquidity, pre-ownership
    legacy rows) keep the existing provenance-clause posture; they already
    cannot fill.
    """
    eligible_owner = (
        select(User.id)
        .where(
            User.id == order.owner_user_id,
            User.organization_id == order.organization_id,
            User.role.in_((UserRole.BUYER, UserRole.SUPPLIER)),
            User.status == UserStatus.APPROVED,
            User.email_verified.is_(True),
            User.must_change_password.is_(False),
            User.kyc_status != "REJECTED",
            or_(
                User.kyc_organization_id.is_(None),
                User.kyc_organization_id == order.organization_id,
            ),
            select(Organization.id)
            .where(
                Organization.id == order.organization_id,
                Organization.verification_status == "APPROVED",
            )
            .correlate(order)
            .exists(),
        )
        .exists()
    )
    assisted_owner = (
        select(User.id)
        .where(
            order.creation_method == OrderCreationMethod.MARKET_SUPPORT,
            User.id == order.owner_user_id,
            User.id == order.created_by_actor_user_id,
            User.role == UserRole.ADMIN,
            User.status == UserStatus.APPROVED,
            select(Organization.id)
            .where(
                Organization.id == order.organization_id,
                Organization.verification_status == "APPROVED",
                Organization.provenance == OrganizationProvenance.REAL.value,
            )
            .correlate(order)
            .exists(),
        )
        .exists()
    )
    return case(
        (order.creation_method == OrderCreationMethod.MARKET_SUPPORT, assisted_owner),
        else_=or_(order.owner_user_id.is_(None), eligible_owner),
    )


def market_data_eligible_organization_clause(organization):
    return and_(
        organization.verification_status == "APPROVED",
        organization.provenance == OrganizationProvenance.REAL.value,
    )


def market_data_eligible_trade_clauses(buyer_organization, seller_organization):
    return (
        market_data_eligible_organization_clause(buyer_organization),
        market_data_eligible_organization_clause(seller_organization),
    )


def canonical_market_product_expression(product):
    """Strict SQL identity for the four active canonical catalog products.

    Public market queries deliberately do not infer a canonical product from
    legacy grades or inactive aliases such as ``Methanol Green``.
    """
    return case(
        *(
            (
                and_(
                    product.id == spec.id,
                    product.name == spec.name,
                    product.fuel_type == spec.fuel_type,
                    product.fuel_grade == spec.fuel_grade,
                ),
                spec.market_product.value,
            )
            for spec in CANONICAL_PRODUCTS
        ),
        else_=None,
    )


def canonical_product_clause(product):
    return and_(
        product.is_active.is_(True),
        or_(
            *(
                and_(
                    product.id == spec.id,
                    product.name == spec.name,
                    product.fuel_type == spec.fuel_type,
                    product.fuel_grade == spec.fuel_grade,
                )
                for spec in CANONICAL_PRODUCTS
            )
        ),
    )


def canonical_delivery_point_clause(delivery_point):
    return and_(
        delivery_point.is_active.is_(True),
        or_(
            *(
                and_(
                    delivery_point.id == spec.id,
                    delivery_point.name == spec.name,
                    delivery_point.region == spec.region,
                )
                for spec in CANONICAL_DELIVERY_POINTS
            )
        ),
    )


def active_market_catalog_clauses(product, delivery_point):
    return (
        canonical_product_clause(product),
        canonical_delivery_point_clause(delivery_point),
    )


def current_public_order_clause(order, *, now_expression=None):
    """Keep public order collections current without inventing a REAL lifetime.

    Synthetic DEMO liquidity must always carry and satisfy an explicit expiry.
    Existing REAL/UNKNOWN user rows may remain visible with a null expiry until
    product owners approve a general order-lifetime policy. Currency alone is
    not enough: the owner-admission mirror below is part of every public
    collection, so a rejected owner's inert orders never surface.
    """
    now_value = now_expression if now_expression is not None else func.now()
    return and_(
        or_(
            order.expires_at > now_value,
            and_(
                order.expires_at.is_(None),
                order.provenance.in_(
                    (
                        OrganizationProvenance.REAL.value,
                        OrganizationProvenance.UNKNOWN.value,
                    )
                ),
            ),
        ),
        # Every public collection also refuses inert owner-rejected liquidity.
        public_order_owner_admission_clause(order),
    )


def public_order_collection_provenance_clause(order):
    """Admit only independently labelled REAL and DEMO public rows."""
    return order.provenance.in_(
        (
            OrganizationProvenance.REAL.value,
            OrganizationProvenance.DEMO.value,
        )
    )
