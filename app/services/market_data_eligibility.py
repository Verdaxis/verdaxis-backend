"""SQL predicates for formal market evidence.

Only approved organizations with REAL immutable provenance are eligible for
formal evidence. UNKNOWN remains quarantined. No name/domain heuristics.
"""
from __future__ import annotations

from datetime import datetime
from uuid import UUID

from sqlalchemy import and_, case, func, or_, select
from sqlalchemy.ext.asyncio import AsyncSession

from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS
from app.models.catalog import DeliveryPoint, Product
from app.models.marketplace import InventoryItem
from app.models.orderbook import (
    OrderBookOrder,
    OrderBookStatus,
    OrderCreationMethod,
    OrderSide,
    Trade,
)
from app.models.port import Port
from app.services.execution_policy import biofuel_supplier_metadata_clause
from app.services.market_provenance import (
    MarketEvidenceScope,
    canonical_availability_window_clause,
    organization_evidence_clause,
    public_trade_evidence_clause,
)
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
    """Strict SQL identity for the active canonical catalog products.

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
        or_(
            order.side != OrderSide.ASK,
            biofuel_supplier_metadata_clause(order, order.product_id),
        ),
    )


def public_order_collection_provenance_clause(order):
    """Admit only independently labelled REAL and DEMO public rows."""
    return order.provenance.in_(
        (
            OrganizationProvenance.REAL.value,
            OrganizationProvenance.DEMO.value,
        )
    )


def _public_order_projection_statement(*, as_of: datetime | None = None):
    """Return the broad public order-book projection shared by refresh checks."""
    return (
        select(OrderBookOrder.id)
        .join(Product, Product.id == OrderBookOrder.product_id)
        .join(DeliveryPoint, DeliveryPoint.id == OrderBookOrder.delivery_point_id)
        .where(
            OrderBookOrder.status.in_(
                (OrderBookStatus.OPEN, OrderBookStatus.PARTIALLY_FILLED)
            ),
            *active_market_catalog_clauses(Product, DeliveryPoint),
            canonical_availability_window_clause(OrderBookOrder.availability_window),
            current_public_order_clause(OrderBookOrder, now_expression=as_of),
            public_order_collection_provenance_clause(OrderBookOrder),
            or_(
                OrderBookOrder.side != OrderSide.ASK,
                and_(
                    func.length(
                        func.trim(func.coalesce(OrderBookOrder.certification_scheme, ""))
                    )
                    > 0,
                    OrderBookOrder.certification_declared.is_(True),
                    func.length(
                        func.trim(func.coalesce(OrderBookOrder.specification_standard, ""))
                    )
                    > 0,
                    OrderBookOrder.msds_available.is_(True),
                    OrderBookOrder.carbon_intensity_gco2_mj.is_not(None),
                    func.length(func.trim(func.coalesce(OrderBookOrder.feedstock, "")))
                    > 0,
                    func.length(func.trim(func.coalesce(OrderBookOrder.origin, ""))) > 0,
                ),
            ),
        )
    )


def _public_inventory_projection_statement():
    """Return exact REAL/DEMO inventory rows used by public availability."""
    real_evidence = organization_evidence_clause(
        Organization, User, MarketEvidenceScope.REAL
    )
    demo_evidence = organization_evidence_clause(
        Organization, User, MarketEvidenceScope.DEMO
    )
    return (
        select(InventoryItem.id)
        .join(Port, Port.id == InventoryItem.port_id)
        .join(DeliveryPoint, DeliveryPoint.name == Port.name)
        .join(Product, Product.name == InventoryItem.product_name)
        .join(Organization, Organization.id == InventoryItem.supplier_id)
        .join(
            User,
            and_(
                User.id == InventoryItem.owner_user_id,
                User.organization_id == Organization.id,
            ),
        )
        .where(
            Port.is_active.is_(True),
            *active_market_catalog_clauses(Product, DeliveryPoint),
            biofuel_supplier_metadata_clause(InventoryItem, Product.id),
            or_(real_evidence, demo_evidence),
        )
    )


async def public_order_is_visible(
    db: AsyncSession,
    order_id: UUID,
    *,
    as_of: datetime | None = None,
) -> bool:
    statement = _public_order_projection_statement(as_of=as_of).where(
        OrderBookOrder.id == order_id
    )
    return (await db.execute(statement.limit(1))).scalar_one_or_none() is not None


async def public_inventory_is_visible(db: AsyncSession, item_id: UUID) -> bool:
    statement = _public_inventory_projection_statement().where(
        InventoryItem.id == item_id
    )
    return (await db.execute(statement.limit(1))).scalar_one_or_none() is not None


async def public_trade_is_visible(db: AsyncSession, trade_id: UUID) -> bool:
    statement = select(Trade.id).where(
        Trade.id == trade_id,
        public_trade_evidence_clause(Trade),
    )
    return (await db.execute(statement.limit(1))).scalar_one_or_none() is not None


async def organization_has_public_market_projection(
    db: AsyncSession, organization_id: UUID
) -> bool:
    order_statement = _public_order_projection_statement().where(
        OrderBookOrder.organization_id == organization_id
    )
    if (await db.execute(order_statement.limit(1))).scalar_one_or_none() is not None:
        return True
    inventory_statement = _public_inventory_projection_statement().where(
        InventoryItem.supplier_id == organization_id
    )
    return (
        await db.execute(inventory_statement.limit(1))
    ).scalar_one_or_none() is not None


async def user_has_public_market_projection(db: AsyncSession, user_id: UUID) -> bool:
    order_statement = _public_order_projection_statement().where(
        OrderBookOrder.owner_user_id == user_id
    )
    if (await db.execute(order_statement.limit(1))).scalar_one_or_none() is not None:
        return True
    inventory_statement = _public_inventory_projection_statement().where(
        InventoryItem.owner_user_id == user_id
    )
    return (
        await db.execute(inventory_statement.limit(1))
    ).scalar_one_or_none() is not None
