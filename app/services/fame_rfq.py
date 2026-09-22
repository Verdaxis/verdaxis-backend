"""One compatibility check for UCOME quotes at submission and revision."""
from datetime import UTC, date, datetime, time, timedelta
from decimal import Decimal
from zoneinfo import ZoneInfo

from app.schemas.fame import FameContractTerms, FameOfferTerms


def delivery_deadline(delivery_end: date) -> datetime:
    """The delivery date includes that whole calendar day in Singapore."""
    return datetime.combine(
        delivery_end + timedelta(days=1), time.min, tzinfo=ZoneInfo("Asia/Singapore")
    ).astimezone(UTC)


def quote_compatibility_errors(
    contract: FameContractTerms,
    offer: FameOfferTerms,
    quantity_mt: Decimal,
) -> list[str]:
    """Check supplier declarations, without certifying the fuel or its evidence."""
    errors = []
    if offer.standard != contract.standard or offer.standard_edition != contract.standard_edition:
        errors.append("Declared fuel standard and edition must match the RFQ")
    if offer.sustainability_scheme != contract.sustainability_scheme:
        errors.append("Declared sustainability scheme must match the RFQ")
    # This pilot has no partial allocation or acceptance workflow.
    if offer.available_quantity_mt < max(quantity_mt, contract.min_fill_mt):
        errors.append("Available quantity must cover the full RFQ quantity")
    if contract.max_cfpp_c is not None and (
        offer.cfpp_c is None or offer.cfpp_c > contract.max_cfpp_c
    ):
        errors.append("Declared CFPP must meet the RFQ maximum")
    if contract.max_ci_gco2e_mj is not None and (
        offer.ci_gco2e_mj is None or offer.ci_gco2e_mj > contract.max_ci_gco2e_mj
    ):
        errors.append("Declared carbon intensity must meet the RFQ maximum")
    if offer.certificate_valid_until < contract.delivery_end:
        errors.append("Declared scheme certificate must remain valid through delivery")
    return errors


def quote_expiry(
    requested: datetime | None,
    rfq_expires_at: datetime,
    *,
    structured: bool,
    now: datetime | None = None,
) -> datetime:
    if structured and requested is None:
        raise ValueError("expires_at is required for a UCOME B100 firm quote")
    expires_at = requested or rfq_expires_at
    if expires_at <= (now or datetime.now(UTC)):
        raise ValueError("Quote expiry must be in the future")
    if expires_at > rfq_expires_at:
        raise ValueError("Quote expiry must not exceed the RFQ deadline")
    return expires_at.astimezone(UTC)
