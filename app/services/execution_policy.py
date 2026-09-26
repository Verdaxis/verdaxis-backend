from __future__ import annotations

from uuid import UUID

from app.market_catalog import BIOFUEL_SPECIFICATION_STANDARDS, PRODUCTS_BY_ID
from app.models.orderbook import OrderBookOrder, OrderCreationMethod, OrderSide
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from sqlalchemy import and_, func, or_, select


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
    )


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
