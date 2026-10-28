from __future__ import annotations

from datetime import datetime
from enum import Enum
from uuid import UUID

from app.market_catalog import (
    BIO_LNG_PRODUCT_ID,
    BIOFUEL_SPECIFICATION_STANDARDS,
    PRODUCTS_BY_ID,
    product_delivery_point_allowed,
)
from app.models.orderbook import OrderBookOrder, OrderCreationMethod, OrderSide
from app.models.user import Organization, OrganizationProvenance, User, UserRole, UserStatus
from app.services.action_policy import ReasonCode
from app.services.kyc_execution import (
    KYC_REJECTED,
    ExecutionEligibilityReason,
    KycExecutionEligibility,
    advisory_kyc_allows,
    customer_organization_is_execution_eligible,
    evaluate_kyc_execution,
)
from app.services.lng_terms import is_bio_lng_product, lng_terms_compatible, stored_order_terms_valid
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
    # another organization revokes access. EXECUTION also falls back to this
    # rule when the KYC execution gate is off.
    INDICATIVE = "INDICATIVE"


def user_account_denial(user: User | None) -> ReasonCode | None:
    """Return why a user account is not admitted to market actions, or ``None``.

    The user-only admission checks, in the order they are applied; the first
    denial wins. No database access.
    """
    if user is None:
        return ReasonCode.ACCOUNT_NOT_APPROVED
    role = getattr(user, "role", None)
    if role == UserRole.ADMIN:
        # There is no established acting-as contract. Admin identity must not
        # be inferred as a buyer or supplier from endpoint fall-through.
        return ReasonCode.ADMIN_CANNOT_EXECUTE
    if getattr(user, "id", None) is None:
        return ReasonCode.ACCOUNT_NOT_APPROVED
    if getattr(user, "organization_id", None) is None:
        return ReasonCode.ORGANIZATION_REQUIRED
    if role not in {UserRole.BUYER, UserRole.SUPPLIER}:
        return ReasonCode.ROLE_NOT_ASSIGNED
    if getattr(user, "status", None) != UserStatus.APPROVED:
        return ReasonCode.ACCOUNT_NOT_APPROVED
    if not getattr(user, "email_verified", False):
        return ReasonCode.EMAIL_NOT_VERIFIED
    if getattr(user, "must_change_password", False):
        return ReasonCode.PASSWORD_CHANGE_REQUIRED
    return None


def _user_account_is_admitted(user: User | None) -> bool:
    return user_account_denial(user) is None


def organization_denial(user: User, organization: Organization | None) -> ReasonCode | None:
    """Return why the user's organization does not admit market actions, or ``None``."""
    organization_id = getattr(organization, "id", None) if organization else None
    if organization_id is None or organization_id != user.organization_id:
        return ReasonCode.ORGANIZATION_REQUIRED
    if getattr(organization, "verification_status", None) != "APPROVED":
        return ReasonCode.ORGANIZATION_NOT_APPROVED
    return None


def _organization_admits(user: User, organization: Organization | None) -> bool:
    return organization_denial(user, organization) is None


def account_is_admitted(user: User | None, organization: Organization | None) -> bool:
    """Account and tenant admission, independent of KYC."""
    return _user_account_is_admitted(user) and _organization_admits(user, organization)


def advisory_kyc_denial(user: User) -> ReasonCode | None:
    """Reason the advisory KYC rule (``advisory_kyc_allows``) refuses a user."""
    if advisory_kyc_allows(user):
        return None
    if getattr(user, "kyc_status", None) == KYC_REJECTED:
        return ReasonCode.KYC_REJECTED
    return ReasonCode.KYC_ORGANIZATION_MISMATCH


def kyc_execution_denial(eligibility: KycExecutionEligibility) -> ReasonCode | None:
    """Map the KYC execution decision to a reason code.

    ``KYC_REQUIRED`` is the code of the 403 the execution gate returns. A human
    rejection keeps its own, more specific code.
    """
    if eligibility.eligible:
        return None
    if eligibility.reason == ExecutionEligibilityReason.KYC_REJECTED:
        return ReasonCode.KYC_REJECTED
    if eligibility.reason == ExecutionEligibilityReason.NOT_APPLICABLE:
        return ReasonCode.ROLE_NOT_ASSIGNED
    return ReasonCode.KYC_REQUIRED


async def market_party_denial(
    db,
    *,
    user: User | None,
    organization: Organization | None = None,
    policy: MarketPartyPolicy,
    now: datetime | None = None,
) -> ReasonCode | None:
    """Return why a party may not act under the policy, or ``None``.

    The single definition of market-party eligibility: ``market_party_is_eligible``
    is exactly ``denial is None``, and the abilities payload reports the code.
    """
    denial = user_account_denial(user)
    if denial is not None:
        return denial
    if policy == MarketPartyPolicy.INDICATIVE:
        denial = advisory_kyc_denial(user)
        if denial is not None:
            return denial
    if organization is None and hasattr(db, "execute"):
        result = await db.execute(select(Organization).where(Organization.id == user.organization_id))
        organization = result.scalar_one_or_none()
    denial = organization_denial(user, organization)
    if denial is not None:
        return denial
    if policy == MarketPartyPolicy.EXECUTION:
        return kyc_execution_denial(await evaluate_kyc_execution(db, user, now=now))
    return None


async def market_party_is_eligible(
    db,
    *,
    user: User | None,
    organization: Organization | None = None,
    policy: MarketPartyPolicy,
    now: datetime | None = None,
) -> bool:
    """Return true only for an admitted party that meets the policy's KYC rule."""
    return (
        await market_party_denial(
            db, user=user, organization=organization, policy=policy, now=now
        )
        is None
    )


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
    return await customer_organization_is_execution_eligible(
        db, order.organization_id, lock_member=True
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


def scalar_carbon_intensity_required(product_id: UUID | None) -> bool:
    """BIO_LNG keeps its certified GHG value in lng_terms; its scalar CI stays NULL."""
    return not is_bio_lng_product(product_id)


def ask_carbon_intensity_clause(record, product_id):
    """SQL mirror of ``scalar_carbon_intensity_required`` for public ASK filters."""
    return or_(
        product_id == BIO_LNG_PRODUCT_ID,
        record.carbon_intensity_gco2_mj.is_not(None),
    )


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
    product_id = getattr(order, "product_id", None)
    if not product_delivery_point_allowed(product_id, getattr(order, "delivery_point_id", None)):
        return False
    if is_bio_lng_product(product_id) and not stored_order_terms_valid(
        getattr(order, "side", None), getattr(order, "lng_terms", None)
    ):
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
        if is_bio_lng_product(product_id) and not order.msds_available:
            return False

    return True


def orders_execution_compatible(left: OrderBookOrder, right: OrderBookOrder) -> bool:
    if not order_is_execution_qualified(left) or not order_is_execution_qualified(right):
        return False

    bid_order = left if getattr(left, "side", None) == OrderSide.BID else right
    ask_order = right if bid_order is left else left

    if is_bio_lng_product(bid_order.product_id) or is_bio_lng_product(ask_order.product_id):
        return lng_terms_compatible(bid_order.lng_terms, ask_order.lng_terms)

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
