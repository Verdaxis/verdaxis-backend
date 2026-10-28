"""Shared validation and projections for non-executable supplier offers."""

from datetime import UTC, datetime
from zoneinfo import ZoneInfo

from fastapi import HTTPException
from sqlalchemy import and_, or_, select

from app.models.catalog import DeliveryPoint, Product
from app.models.supplier_offer import SupplierOffer
from app.models.user import (
    Organization,
    OrganizationProvenance,
    User,
    UserRole,
    UserStatus,
)
from app.schemas.supplier_offer import (
    FameFuelSummary,
    SupplierOfferCreate,
    SupplierOfferResponse,
    SupplierOfferSnapshot,
    SupplierOfferPublicSnapshot,
    SupplierOfferTerms,
)
from app.services.availability_windows import is_tradable_availability_window
from app.services.fame_rfq import delivery_deadline
from app.services.market_data_eligibility import (
    canonical_delivery_point_clause,
    canonical_product_clause,
)


def public_offer_owner_clause():
    """Hide offers immediately when the recorded publisher loses admission."""
    return (
        select(User.id)
        .join(Organization, User.organization_id == Organization.id)
        .where(
            User.id == SupplierOffer.supplier_user_id,
            User.organization_id == SupplierOffer.supplier_org_id,
            User.role == UserRole.SUPPLIER,
            User.status == UserStatus.APPROVED,
            User.email_verified.is_(True),
            User.must_change_password.is_(False),
            or_(User.kyc_status.is_(None), User.kyc_status != "REJECTED"),
            or_(
                User.kyc_organization_id.is_(None),
                User.kyc_organization_id == User.organization_id,
            ),
            Organization.verification_status == "APPROVED",
            Organization.provenance == OrganizationProvenance.REAL,
        )
        .exists()
    )


def public_offer_clause():
    active_product = (
        select(Product.id)
        .where(
            Product.id == SupplierOffer.product_id,
            canonical_product_clause(Product),
        )
        .exists()
    )
    active_delivery_point = (
        select(DeliveryPoint.id)
        .where(
            DeliveryPoint.id == SupplierOffer.delivery_point_id,
            canonical_delivery_point_clause(DeliveryPoint),
        )
        .exists()
    )
    return and_(
        SupplierOffer.status == "OPEN",
        SupplierOffer.expires_at > datetime.now(UTC),
        active_product,
        active_delivery_point,
        public_offer_owner_clause(),
    )


def validate_offer_dates(
    payload: SupplierOfferCreate, *, now: datetime | None = None
) -> None:
    now = now or datetime.now(UTC)
    today = now.astimezone(ZoneInfo("Asia/Singapore")).date()
    if payload.delivery_start < today:
        raise HTTPException(
            status_code=422, detail="Delivery start must not be in the past"
        )
    if payload.expires_at <= now or payload.expires_at > delivery_deadline(
        payload.delivery_end
    ):
        raise HTTPException(
            status_code=422,
            detail="Offer expiry must be future and on or before the delivery end date in Singapore",
        )
    if not is_tradable_availability_window(payload.availability_window):
        raise HTTPException(
            status_code=422, detail="Availability window is no longer open"
        )
    quality = payload.fuel_terms.quality_evidence
    if quality and any(
        value is not None and value > today
        for value in (quality.sampled_on, quality.tested_on)
    ):
        raise HTTPException(
            status_code=422,
            detail="Quality sampling and test dates must not be in the future",
        )


def listing_terms_payload(payload: SupplierOfferCreate) -> dict:
    return payload.model_dump(mode="json", include=set(SupplierOfferTerms.model_fields))


def offer_snapshot(offer: SupplierOffer) -> SupplierOfferSnapshot:
    return SupplierOfferSnapshot(
        offer_id=offer.id,
        revision=offer.revision,
        supplier_org_id=offer.supplier_org_id,
        product_id=offer.product_id,
        delivery_point_id=offer.delivery_point_id,
        quantity_mt=offer.quantity_mt,
        min_fill_mt=offer.min_fill_mt,
        price_per_mt_usd=offer.price_per_mt_usd,
        availability_window=offer.availability_window,
        expires_at=offer.expires_at,
        listing_terms=SupplierOfferTerms.model_validate(offer.listing_terms),
    )


def public_offer_snapshot(
    snapshot: SupplierOfferSnapshot,
) -> SupplierOfferPublicSnapshot:
    payload = snapshot.model_dump(exclude={"supplier_org_id"})
    payload["listing_terms"]["fuel_terms"] = FameFuelSummary.model_validate(
        snapshot.listing_terms.fuel_terms.model_dump()
    )
    return SupplierOfferPublicSnapshot.model_validate(payload)


def offer_response(
    offer: SupplierOffer,
    viewer: User,
    *,
    viewer_is_eligible: bool,
    viewer_can_inquire: bool = False,
) -> SupplierOfferResponse:
    own_org = viewer.organization_id == offer.supplier_org_id
    creator = own_org and viewer.id == offer.supplier_user_id
    open_offer = offer.status == "OPEN" and offer.expires_at > datetime.now(UTC)
    terms = SupplierOfferTerms.model_validate(offer.listing_terms)
    payload = terms.model_dump()
    if not own_org:
        # A separate response type makes future private fields opt-in, rather
        # than relying on a list of keys that can become incomplete.
        payload["fuel_terms"] = FameFuelSummary.model_validate(
            terms.fuel_terms.model_dump()
        )
    return SupplierOfferResponse(
        **payload,
        id=offer.id,
        product_id=offer.product_id,
        delivery_point_id=offer.delivery_point_id,
        quantity_mt=offer.quantity_mt,
        min_fill_mt=offer.min_fill_mt,
        price_per_mt_usd=offer.price_per_mt_usd,
        availability_window=offer.availability_window,
        status="EXPIRED" if offer.status == "OPEN" and not open_offer else offer.status,
        revision=offer.revision,
        expires_at=offer.expires_at,
        created_at=offer.created_at,
        updated_at=offer.updated_at,
        can_edit=creator and viewer_is_eligible and viewer.role == UserRole.SUPPLIER,
        can_withdraw=creator and offer.status == "OPEN",
        can_request_quote=not own_org and open_offer and viewer_can_inquire,
    )
