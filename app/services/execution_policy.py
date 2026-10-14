from __future__ import annotations

from datetime import datetime
from enum import Enum
from uuid import UUID

from app.market_catalog import BIOFUEL_SPECIFICATION_STANDARDS, PRODUCTS_BY_ID
from app.models.orderbook import OrderBookOrder, OrderCreationMethod, OrderSide
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from app.services.kyc_execution import (
    KYC_REJECTED,
    customer_organization_is_execution_eligible,
    evaluate_kyc_execution,
)
from sqlalchemy import and_, func, or_, select


class MarketPartyPolicy(str, Enum):
    """Which KYC condition a market action adds to account admission."""

    # Creates or confirms an economic commitment: the KYC execution rule.
    EXECUTION = "EXECUTION"
    # Settles or withdraws an existing commitment (cancel, decline, deliver,
    # pay): never blocked by KYC.
    CLEANUP = "CLEANUP"
    # Non-executable surfaces (RFQ, negotiation, inventory, suggestions): the
    # earlier advisory KYC, in which only a rejection or a review bound to
    # another organization revokes access. Unchanged by the KYC gate.
    INDICATIVE = "INDICATIVE"


def _user_account_is_admitted(user: User | None) -> bool:
    if getattr(user, "id", None) is None or getattr(user, "organization_id", None) is None:
        return False
    if getattr(user, "role", None) not in {UserRole.BUYER, UserRole.SUPPLIER}:
        return False
    if getattr(user, "status", None) != UserStatus.APPROVED or not getattr(user, "email_verified", False):
        return False
    return not getattr(user, "must_change_password", False)


def _organization_admits(user: User, organization: Organization | None) -> bool:
    organization_id = getattr(organization, "id", None) if organization else None
    return (
        organization_id is not None
        and organization_id == user.organization_id
        and getattr(organization, "verification_status", None) == "APPROVED"
    )


def account_is_admitted(user: User | None, organization: Organization | None) -> bool:
    """Account and tenant admission, independent of KYC."""
    return _user_account_is_admitted(user) and _organization_admits(user, organization)


def advisory_kyc_allows(user: User) -> bool:
    """The pre-gate advisory KYC kept for indicative surfaces."""
    if getattr(user, "kyc_status", None) == KYC_REJECTED:
        return False
    kyc_organization_id = getattr(user, "kyc_organization_id", None)
    return kyc_organization_id is None or kyc_organization_id == user.organization_id


async def market_party_is_eligible(
    db,
    *,
    user: User | None,
    organization: Organization | None = None,
    policy: MarketPartyPolicy,
    now: datetime | None = None,
) -> bool:
    """Return true only for an admitted party that meets the policy's KYC rule."""
    if not _user_account_is_admitted(user):
        return False
    if policy == MarketPartyPolicy.INDICATIVE and not advisory_kyc_allows(user):
        return False
    if organization is None and hasattr(db, "execute"):
        result = await db.execute(select(Organization).where(Organization.id == user.organization_id))
        organization = result.scalar_one_or_none()
    if not _organization_admits(user, organization):
        return False
    if policy == MarketPartyPolicy.EXECUTION:
        return (await evaluate_kyc_execution(db, user, now=now)).eligible
    return True


async def execution_party_is_eligible(
    db,
    *,
    user: User | None,
    organization: Organization | None = None,
    now: datetime | None = None,
) -> bool:
    """Admission plus the KYC execution rule, for new economic commitments."""
    return await market_party_is_eligible(
        db,
        user=user,
        organization=organization,
        policy=MarketPartyPolicy.EXECUTION,
        now=now,
    )


async def order_owner_is_execution_eligible(
    db,
    *,
    order: OrderBookOrder,
    user: User | None,
    organization: Organization | None,
    policy: MarketPartyPolicy = MarketPartyPolicy.EXECUTION,
) -> bool:
    """Revalidate either a self-service principal or an assisted-order actor."""
    if order.creation_method != OrderCreationMethod.MARKET_SUPPORT:
        return await market_party_is_eligible(
            db, user=user, organization=organization, policy=policy
        )
    assisted_owner_admitted = bool(
        user
        and organization
        and user.id == order.created_by_actor_user_id == order.owner_user_id
        and user.role == UserRole.ADMIN
        and user.status == UserStatus.APPROVED
        and organization.id == order.organization_id
        and organization.verification_status == "APPROVED"
        and organization.provenance == OrganizationProvenance.REAL
    )
    if not assisted_owner_admitted or policy != MarketPartyPolicy.EXECUTION:
        return assisted_owner_admitted
    # Assisted execution acts for the customer organization; the support
    # administrator's own KYC is irrelevant.
    return await customer_organization_is_execution_eligible(db, order.organization_id)


def normalize_certification_scheme(value: str | None) -> str | None:
    if value is None:
        return None
    normalized = value.strip().upper()
    return normalized or None


def supplier_product_metadata_error(
    product_id: UUID | None,
    *,
    specification_standard: str | None,
    carbon_intensity_method: str | None,
) -> str | None:
    """Check fixed biofuel contracts without changing other product terms."""
    required_standard = BIOFUEL_SPECIFICATION_STANDARDS.get(product_id)
    if required_standard is None:
        return None
    product_name = PRODUCTS_BY_ID[product_id].name
    if (specification_standard or "").strip().upper() != required_standard:
        return f"{product_name} ASK orders require specification_standard {required_standard}"
    if not (carbon_intensity_method or "").strip():
        return f"{product_name} ASK orders require a lifecycle carbon_intensity_method for the whole supplied fuel"
    return None


def biofuel_supplier_metadata_clause(record, product_id):
    """SQL mirror for fixed biofuel contracts in public market projections."""
    return or_(
        product_id.not_in(tuple(BIOFUEL_SPECIFICATION_STANDARDS)),
        and_(
            record.off_spec.is_(False),
            or_(*(
                and_(
                    product_id == identity,
                    func.upper(func.trim(record.specification_standard, " \t\r\n\f\v")) == standard,
                )
                for identity, standard in BIOFUEL_SPECIFICATION_STANDARDS.items()
            )),
            func.length(func.trim(record.carbon_intensity_method, " \t\r\n\f\v")) > 0,
            record.certification_declared.is_(True),
            func.length(func.trim(record.certification_scheme, " \t\r\n\f\v")) > 0,
            record.msds_available.is_(True),
            record.carbon_intensity_gco2_mj.is_not(None),
            func.length(func.trim(record.feedstock, " \t\r\n\f\v")) > 0,
            func.length(func.trim(record.origin, " \t\r\n\f\v")) > 0,
        ),
    )


def order_is_execution_qualified(order: OrderBookOrder) -> bool:
    if getattr(order, "off_spec", False):
        return False

    normalized_scheme = normalize_certification_scheme(getattr(order, "certification_scheme", None))

    if getattr(order, "side", None) == OrderSide.ASK:
        if normalized_scheme is None:
            return False
        if not bool(getattr(order, "certification_declared", False)):
            return False
        if getattr(order, "product_id", None) in BIOFUEL_SPECIFICATION_STANDARDS:
            if supplier_product_metadata_error(
                order.product_id,
                specification_standard=order.specification_standard,
                carbon_intensity_method=order.carbon_intensity_method,
            ):
                return False
            if not order.msds_available or order.carbon_intensity_gco2_mj is None:
                return False
            if not (order.feedstock or "").strip() or not (order.origin or "").strip():
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
