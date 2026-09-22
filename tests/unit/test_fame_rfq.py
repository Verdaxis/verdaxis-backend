"""UCOME declarations, compatibility, expiry and supplier quote lifecycle."""
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from inspect import unwrap
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock
from uuid import uuid4

from fastapi import HTTPException, Request
from pydantic import ValidationError
import pytest

from app.models.rfq import QuoteStatus, RFQQuote, RFQStatus
from app.models.user import UserRole
from app.routers import rfq as router
from app.schemas.fame import FameContractTerms, FameOfferTerms
from app.schemas.rfq import RFQCreateRequest, RFQQuoteRequest, RFQQuoteRevisionRequest
from app.services.fame_rfq import delivery_deadline, quote_compatibility_errors, quote_expiry
from app.services import market_admission


@pytest.fixture
def contract_data():
    start = date.today() + timedelta(days=20)
    return {
        "neat_fame": True, "standard": "EN_14214", "standard_edition": "2012+A2:2019",
        "max_cfpp_c": -5, "max_ci_gco2e_mj": 20,
        "delivery_basis": "EX_TANK", "named_location": "Singapore, terminal to be agreed",
        "delivery_start": start.isoformat(), "delivery_end": (start + timedelta(days=5)).isoformat(),
        "quantity_tolerance_pct": 5, "min_fill_mt": 100,
        "payment_terms": "Payment before loading", "inspection_terms": "Independent loading inspection",
        "title_risk_terms": "Title and risk pass at loading flange", "claims_terms": "Claims within 30 days",
        "sustainability_scheme": "ISCC_EU", "evidence_due": "BEFORE_LOADING",
    }


@pytest.fixture
def offer_data(contract_data):
    return {
        "matches_contract_terms": True, "batch_reference": "Future production lot September",
        "producing_site": "Plant A", "production_origin": "Malaysia", "feedstock_origin": "Malaysia",
        "shipping_location": "Singapore terminal A", "uco_mass_pct": 100,
        "standard": contract_data["standard"], "standard_edition": contract_data["standard_edition"],
        "cfpp_c": -6, "ci_gco2e_mj": 14, "ci_methodology": "RED actual value, supplier declaration",
        "sustainability_scheme": "ISCC_EU", "certificate_reference": "ISCC-declared-reference",
        "certificate_holder": "Supplier Ltd", "certificate_valid_until": contract_data["delivery_end"],
        "evidence_status": "PENDING", "document_references": [], "available_quantity_mt": 500,
    }


def test_pending_future_batch_is_a_declaration_and_serializes(contract_data, offer_data):
    contract = FameContractTerms.model_validate(contract_data)
    offer = FameOfferTerms.model_validate(offer_data)
    assert not quote_compatibility_errors(contract, offer, Decimal("500"))
    assert offer.model_dump(mode="json")["evidence_status"] == "PENDING"
    assert "verified" not in offer.model_dump()


@pytest.mark.parametrize("change", [
    {"neat_fame": False}, {"standard": "ISO_8217"}, {"delivery_basis": "DAP"},
    {"named_location": "   "}, {"max_ci_gco2e_mj": "NaN"},
    {"delivery_end": "2020-01-01"}, {"blend_pct": 30},
])
def test_contract_rejects_ambiguous_or_invalid_fuel_terms(contract_data, change):
    with pytest.raises(ValidationError):
        FameContractTerms.model_validate(contract_data | change)


def test_minimum_fill_cannot_exceed_rfq_quantity(contract_data):
    with pytest.raises(ValidationError, match="min_fill_mt"):
        RFQCreateRequest(product_id=uuid4(), delivery_point_id=uuid4(), quantity_mt=50,
                         contract_terms=contract_data)


@pytest.mark.parametrize("change", [
    {"uco_mass_pct": 80}, {"matches_contract_terms": False},
    {"evidence_status": "AVAILABLE"}, {"ci_methodology": None},
])
def test_offer_requires_explicit_and_consistent_declarations(offer_data, change):
    with pytest.raises(ValidationError):
        FameOfferTerms.model_validate(offer_data | change)


@pytest.mark.parametrize("change, message", [
    ({"standard_edition": "different edition"}, "standard and edition"),
    ({"sustainability_scheme": "REDCERT_EU"}, "sustainability scheme"),
    ({"available_quantity_mt": 499}, "full RFQ quantity"),
    ({"cfpp_c": None}, "CFPP"),
    ({"cfpp_c": -4}, "CFPP"),
    ({"ci_gco2e_mj": 21}, "carbon intensity"),
    ({"certificate_valid_until": "2020-01-01"}, "valid through delivery"),
])
def test_same_compatibility_predicate_rejects_incompatible_quotes(contract_data, offer_data, change, message):
    errors = quote_compatibility_errors(
        FameContractTerms.model_validate(contract_data),
        FameOfferTerms.model_validate(offer_data | change), Decimal("500"),
    )
    assert any(message in error for error in errors)


def test_quote_expiry_is_explicit_bounded_and_timezone_aware():
    now = datetime.now(UTC)
    deadline = now + timedelta(hours=24)
    assert quote_expiry(None, deadline, structured=False, now=now) == deadline
    assert quote_expiry(deadline, deadline, structured=True, now=now) == deadline
    for expiry in (None, now, deadline + timedelta(seconds=1)):
        with pytest.raises(ValueError):
            quote_expiry(expiry, deadline, structured=True, now=now)
    with pytest.raises(ValidationError):
        RFQQuoteRequest(price_per_mt_usd=900, expires_at="2026-09-30T12:00:00")


def test_delivery_deadline_uses_whole_singapore_calendar_day():
    assert delivery_deadline(date(2026, 9, 22)) == datetime(2026, 9, 22, 16, tzinfo=UTC)


def test_supplier_history_retains_own_quote_but_hides_unrelated_closed_rfq():
    supplier = SimpleNamespace(role=UserRole.SUPPLIER, organization_id=uuid4())
    rfq = SimpleNamespace(buyer_org_id=uuid4(), status=RFQStatus.CANCELLED,
                          expires_at=datetime.now(UTC) - timedelta(days=1), quotes=[])
    with pytest.raises(HTTPException) as error:
        router._ensure_rfq_detail_visible(rfq, supplier)
    assert error.value.status_code == 404
    rfq.quotes = [SimpleNamespace(seller_org_id=supplier.organization_id)]
    router._ensure_rfq_detail_visible(rfq, supplier)


@pytest.mark.asyncio
async def test_history_response_hides_other_quotes_and_cancel_is_creator_specific():
    creator_id, buyer_id, supplier_id = uuid4(), uuid4(), uuid4()
    now = datetime.now(UTC)
    quotes = [RFQQuote(id=uuid4(), seller_org_id=org_id, price_per_mt_usd=900, status=QuoteStatus.PENDING,
                       created_at=now, expires_at=None, revision=1, offer_terms=None, notes=None)
              for org_id in (supplier_id, uuid4())]
    rfq = SimpleNamespace(id=uuid4(), buyer_org_id=buyer_id, buyer_user_id=creator_id,
                          product_id=uuid4(), delivery_point_id=None, quantity_mt=500, target_price_per_mt=None,
                          availability_window="SPOT", notes=None, is_anonymous=True, status=RFQStatus.QUOTED,
                          expires_at=now + timedelta(hours=1), created_at=now, quotes=quotes, contract_terms=None)
    result = MagicMock()
    result.scalar_one_or_none.return_value = "Organization or product label"
    db = AsyncMock()
    db.execute.return_value = result
    supplier_view = await router._build_rfq_response(db, rfq, viewer_org_id=supplier_id, viewer_user_id=uuid4())
    assert supplier_view.buyer_org_id is None
    assert [quote.seller_org_id for quote in supplier_view.quotes] == [supplier_id]
    assert supplier_view.quotes[0].expires_at == rfq.expires_at
    assert supplier_view.execution_enabled is False
    colleague_view = await router._build_rfq_response(db, rfq, viewer_org_id=buyer_id, viewer_user_id=uuid4())
    assert colleague_view.can_cancel is False
    creator_view = await router._build_rfq_response(db, rfq, viewer_org_id=buyer_id, viewer_user_id=creator_id)
    assert creator_view.can_cancel is True


def quote_context(offer_data, contract_data):
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4())
    quote = RFQQuote(
        id=uuid4(), rfq_id=uuid4(), seller_org_id=user.organization_id,
        seller_user_id=user.id, price_per_mt_usd=Decimal("900"), notes="First version",
        expires_at=datetime.now(UTC) - timedelta(minutes=1), revision=1,
        offer_terms=FameOfferTerms.model_validate(offer_data).model_dump(mode="json"),
        status=QuoteStatus.PENDING, created_at=datetime.now(UTC),
    )
    rfq = SimpleNamespace(
        id=quote.rfq_id, buyer_org_id=uuid4(), quantity_mt=Decimal("500"),
        contract_terms=FameContractTerms.model_validate(contract_data).model_dump(mode="json"),
        expires_at=datetime.now(UTC) + timedelta(hours=24), quotes=[quote], status=RFQStatus.QUOTED,
    )
    return user, rfq, quote


@pytest.mark.asyncio
async def test_revision_renews_expired_quote_and_audits_previous_terms(monkeypatch, offer_data, contract_data):
    user, rfq, quote = quote_context(offer_data, contract_data)
    db = AsyncMock()
    monkeypatch.setattr(router, "_lock_rfq_for_quote_change", AsyncMock(return_value=rfq))
    monkeypatch.setattr(router, "request_audit_context", lambda request: {})
    audit = AsyncMock()
    monkeypatch.setattr(router, "record_audit", audit)
    monkeypatch.setattr(router, "_notify_org_users", AsyncMock())
    enqueue = AsyncMock()
    monkeypatch.setattr(router, "enqueue_market_events", enqueue)
    monkeypatch.setattr(router, "_quote_response", AsyncMock(return_value="response"))
    payload = RFQQuoteRevisionRequest(price_per_mt_usd=925, notes="Revised price", expires_at=rfq.expires_at,
                                      offer_terms=offer_data, expected_revision=1)
    result = await unwrap(router.revise_quote)(Request({"type": "http"}), rfq.id, quote.id,
                                              payload, db, user, None)
    assert result == "response"
    assert quote.revision == 2
    assert quote.price_per_mt_usd == Decimal("925")
    assert quote.expires_at == rfq.expires_at
    history = audit.call_args.kwargs["changes"]
    assert history["from"]["price_per_mt_usd"] == "900"
    assert history["from"]["notes"] == "First version"
    assert history["to"]["revision"] == 2
    assert set(enqueue.call_args.args[1][0].participant_org_ids) == {user.organization_id, rfq.buyer_org_id}
    db.commit.assert_awaited_once()


@pytest.mark.asyncio
async def test_stale_revision_does_not_overwrite_current_quote(monkeypatch, offer_data, contract_data):
    user, rfq, quote = quote_context(offer_data, contract_data)
    quote.revision = 2
    db = AsyncMock()
    monkeypatch.setattr(router, "_lock_rfq_for_quote_change", AsyncMock(return_value=rfq))
    payload = RFQQuoteRevisionRequest(price_per_mt_usd=925, expires_at=rfq.expires_at,
                                      offer_terms=offer_data, expected_revision=1)
    with pytest.raises(HTTPException) as error:
        await unwrap(router.revise_quote)(Request({"type": "http"}), rfq.id, quote.id,
                                         payload, db, user, None)
    assert error.value.status_code == 409
    assert quote.revision == 2
    assert quote.price_per_mt_usd == Decimal("900")
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
async def test_withdrawal_versions_state_and_preserves_idempotence(monkeypatch, offer_data, contract_data):
    user, rfq, quote = quote_context(offer_data, contract_data)
    db = AsyncMock()
    audit = AsyncMock()
    monkeypatch.setattr(router, "_lock_rfq_for_quote_change", AsyncMock(return_value=rfq))
    monkeypatch.setattr(router, "request_audit_context", lambda request: {})
    monkeypatch.setattr(router, "record_audit", audit)
    monkeypatch.setattr(router, "_notify_org_users", AsyncMock())
    monkeypatch.setattr(router, "enqueue_market_events", AsyncMock())
    monkeypatch.setattr(router, "_quote_response", AsyncMock())
    request = Request({"type": "http"})

    await unwrap(router.withdraw_quote)(request, rfq.id, quote.id, db, user)
    assert quote.status == QuoteStatus.WITHDRAWN
    assert quote.revision == 2
    history = audit.call_args.kwargs["changes"]
    assert history["from"]["revision"] == 1
    assert history["to"]["revision"] == 2
    await unwrap(router.withdraw_quote)(request, rfq.id, quote.id, db, user)
    assert quote.revision == 2
    audit.assert_awaited_once()
    db.commit.assert_awaited_once()


@pytest.mark.asyncio
async def test_foreign_supplier_cannot_withdraw_quote(monkeypatch, offer_data, contract_data):
    user, rfq, quote = quote_context(offer_data, contract_data)
    user.organization_id = uuid4()
    db = AsyncMock()
    monkeypatch.setattr(router, "_lock_rfq_for_quote_change", AsyncMock(return_value=rfq))
    with pytest.raises(HTTPException) as error:
        await unwrap(router.withdraw_quote)(Request({"type": "http"}), rfq.id, quote.id, db, user)
    assert error.value.status_code == 404
    assert quote.status == QuoteStatus.PENDING
    db.commit.assert_not_awaited()


@pytest.mark.asyncio
@pytest.mark.parametrize("membership_changed", [False, True])
async def test_withdrawal_admission_retains_membership_check_after_approval_loss(monkeypatch, membership_changed):
    organization_id = uuid4()
    user = SimpleNamespace(id=uuid4(), organization_id=uuid4() if membership_changed else organization_id)
    organization = SimpleNamespace(id=organization_id, verification_status="REJECTED")
    users_result, organizations_result = MagicMock(), MagicMock()
    users_result.scalars.return_value = [user]
    organizations_result.scalars.return_value = [organization]
    db = AsyncMock()
    db.execute.side_effect = [users_result, organizations_result]
    eligibility = AsyncMock(return_value=False)
    monkeypatch.setattr(market_admission, "execution_party_is_eligible", eligibility)
    operation = market_admission.lock_and_load_market_organizations(
        db, [organization_id],
        actor_ownerships=(market_admission.MarketActorOwnership(user.id, organization_id),),
        require_approved=False, require_execution_eligible=False,
    )
    if membership_changed:
        with pytest.raises(HTTPException, match="organization ownership changed"):
            await operation
    else:
        assert await operation == {organization_id: organization}
    eligibility.assert_not_awaited()
