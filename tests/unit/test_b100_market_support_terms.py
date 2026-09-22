"""Assisted B100 orders keep the customer's exact declared fuel terms."""

from copy import deepcopy
from datetime import UTC, date, datetime
from decimal import Decimal
from unittest.mock import AsyncMock, Mock
from uuid import uuid4

import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.models.catalog import DeliveryPoint, Product
from app.models.market_support import MarketSupportAuthorization
from app.models.orderbook import OrderBookOrder, OrderCreationMethod, OrderSide
from app.models.user import (
    Organization,
    OrganizationProvenance,
    User,
    UserRole,
    UserStatus,
)
from app.routers.market_support import (
    _authorization_order,
    _candidate_from_authorization,
)
from app.schemas.fame_order import FameAskTerms
from app.schemas.orderbook import OrderCreate
from app.services.idempotency import idempotency_request_hash
from app.services.market_support import (
    authorization_terms_digest,
    economic_order_idempotency_payload,
    economic_order_payload,
)
from app.services.market_support_post_only import assess_locked_order


@pytest.fixture
def ask_terms():
    return {
        "side": "ASK",
        "neat_fame": True,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "customer-operator-certificate",
        "certificate_holder": "Customer supplier",
        "certificate_valid_until": "2099-01-01",
        "evidence_status": "PENDING",
        "evidence_due": "BEFORE_LOADING",
        "batch_reference": "customer-batch",
        "quality_evidence": {
            "status": "AVAILABLE",
            "reference": "customer-coa",
            "batch_reference": "customer-batch",
            "laboratory": "Customer laboratory",
            "tested_on": "2026-01-01",
            "results": [
                {
                    "property": "KINEMATIC_VISCOSITY_40C_MM2_S",
                    "value": Decimal("4.50"),
                    "method": "EN ISO 3104",
                }
            ],
        },
        "sustainability_evidence": {
            "status": "PENDING",
            "document_type": "POS",
            "quantity_mt": Decimal("100.00"),
            "due": "BEFORE_LOADING",
        },
    }


def _order(*, fame_terms=None) -> OrderCreate:
    return OrderCreate(
        side="ASK",
        product_id=PRODUCT_IDS["UCOME_B100" if fame_terms else "BIO_METHANOL"],
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
        quantity_mt=Decimal("100.00"),
        price_per_mt_usd=Decimal("650.00"),
        availability_window="SPOT",
        expires_at=datetime(2099, 1, 1, tzinfo=UTC),
        certification_declared=True,
        certification_scheme="ISCC EU",
        specification_standard="EN 14214" if fame_terms else "IMPCA",
        msds_available=True,
        fame_terms=fame_terms,
    )


def _authorization(ask_terms) -> MarketSupportAuthorization:
    order = _order(fame_terms=ask_terms)
    return MarketSupportAuthorization(
        id=uuid4(),
        organization_id=uuid4(),
        accountable_user_id=uuid4(),
        created_by_actor_user_id=uuid4(),
        order_side="ASK",
        product_id=order.product_id,
        delivery_point_id=order.delivery_point_id,
        quantity_mt=order.quantity_mt,
        price_per_mt_usd=order.price_per_mt_usd,
        availability_window=order.availability_window,
        order_expires_at=order.expires_at,
        is_anonymous=True,
        certifications=[],
        certification_declared=order.certification_declared,
        certification_scheme=order.certification_scheme,
        specification_standard=order.specification_standard,
        msds_available=order.msds_available,
        carbon_intensity_gco2_mj=None,
        carbon_intensity_method=None,
        feedstock=None,
        origin=None,
        off_spec=False,
        off_spec_notes=None,
        fame_terms=FameAskTerms.model_validate(ask_terms).model_dump(mode="json"),
    )


def _candidate(authorization):
    organization = Organization(
        id=authorization.organization_id,
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    candidate = _candidate_from_authorization(
        authorization,
        organization,
        Product(id=authorization.product_id),
        DeliveryPoint(id=authorization.delivery_point_id),
    )
    return candidate, organization


def test_nested_quality_change_requires_new_authority_and_idempotency(ask_terms):
    original = _order(fame_terms=ask_terms)
    changed_terms = deepcopy(ask_terms)
    changed_terms["quality_evidence"]["results"][0]["value"] = Decimal("4.60")
    changed = _order(fame_terms=changed_terms)

    assert authorization_terms_digest(original) != authorization_terms_digest(changed)
    assert idempotency_request_hash(
        economic_order_idempotency_payload(original)
    ) != idempotency_request_hash(economic_order_idempotency_payload(changed))


def test_nested_decimal_scale_keeps_the_same_authority_and_idempotency(ask_terms):
    original = _order(fame_terms=ask_terms)
    equivalent_terms = deepcopy(ask_terms)
    equivalent_terms["quality_evidence"]["results"][0]["value"] = Decimal("4.5000")
    equivalent_terms["sustainability_evidence"]["quantity_mt"] = Decimal(100)
    equivalent = _order(fame_terms=equivalent_terms)

    assert authorization_terms_digest(original) == authorization_terms_digest(
        equivalent
    )
    assert idempotency_request_hash(
        economic_order_idempotency_payload(original)
    ) == idempotency_request_hash(economic_order_idempotency_payload(equivalent))


def test_fame_terms_are_economic_even_when_certificate_is_only_declared(ask_terms):
    order = _order(fame_terms=ask_terms)
    terms = economic_order_payload(order)["fame_terms"]

    assert terms["certificate_reference"] == "customer-operator-certificate"
    assert terms["certificate_valid_until"] == date(2099, 1, 1)
    assert terms["evidence_status"] == "PENDING"
    assert "certificate_verified" not in terms


def test_generic_orders_omit_absent_fame_terms_from_existing_payloads():
    order = _order()

    assert "fame_terms" not in economic_order_payload(order)
    assert "fame_terms" not in economic_order_idempotency_payload(order)


def test_generic_order_keeps_its_pre_b100_authority_and_idempotency_hashes():
    order = _order()

    # These hashes come from the pre-B100 OrderCreate and support helpers.
    # New optional fields must not invalidate an existing authorization or key.
    assert authorization_terms_digest(order) == (
        "befcaf931c334f7d8d101d3923c9f66fa30aaf027dab5f20cc061926194e8692"
    )
    assert idempotency_request_hash(economic_order_idempotency_payload(order)) == (
        "4e734711288539107aa3d15bec2d8a86f9609daa55c28db3765ae1db05fabf2f"
    )


def test_reconstructed_order_keeps_a_separate_exact_declaration(ask_terms):
    authorization = _authorization(ask_terms)
    saved_terms = deepcopy(authorization.fame_terms)
    order = _authorization_order(authorization)

    assert order.fame_terms.model_dump(mode="json") == saved_terms
    order.fame_terms.quality_evidence.results[0].value = Decimal("9.00")
    order.fame_terms.document_references.append("unapproved-document")
    assert authorization.fame_terms == saved_terms


def test_candidate_keeps_a_deep_copy_without_verifying_the_certificate(ask_terms):
    authorization = _authorization(ask_terms)
    saved_terms = deepcopy(authorization.fame_terms)
    candidate, _ = _candidate(authorization)

    assert candidate.fame_terms == saved_terms
    assert not candidate.is_verdaxis_verified
    assert candidate.fame_terms["evidence_status"] == "PENDING"
    candidate.fame_terms["quality_evidence"]["results"][0]["value"] = "9.00"
    candidate.fame_terms["document_references"].append("unapproved-document")
    assert authorization.fame_terms == saved_terms


def _rows(values):
    result = Mock()
    result.scalars.return_value.all.return_value = values
    return result


@pytest.mark.parametrize(
    "declared_cfpp,would_cross",
    [(None, False), (Decimal(-4), False), (Decimal(-5), True)],
)
async def test_post_only_uses_the_buyer_fuel_requirements(
    ask_terms, declared_cfpp, would_cross
):
    ask_terms["cfpp_c"] = declared_cfpp
    candidate, supplier = _candidate(_authorization(ask_terms))
    buyer = Organization(
        id=uuid4(),
        verification_status="APPROVED",
        provenance=OrganizationProvenance.REAL,
    )
    owner = User(
        id=uuid4(),
        organization_id=buyer.id,
        role=UserRole.BUYER,
        status=UserStatus.APPROVED,
        email_verified=True,
        must_change_password=False,
    )
    crossing = OrderBookOrder(
        id=uuid4(),
        organization_id=buyer.id,
        owner_user_id=owner.id,
        creation_method=OrderCreationMethod.SELF_SERVICE,
        provenance=OrganizationProvenance.REAL,
        side=OrderSide.BID,
        product_id=candidate.product_id,
        delivery_point_id=candidate.delivery_point_id,
        availability_window="SPOT",
        quantity_mt=Decimal(100),
        remaining_quantity_mt=Decimal(100),
        price_per_mt_usd=Decimal(700),
        created_at=datetime(2026, 1, 1, tzinfo=UTC),
        certifications=[],
        certification_scheme=None,
        off_spec=False,
        fame_terms={
            "side": "BID",
            "neat_fame": True,
            "standard": "EN_14214",
            "standard_edition": "2012+A2:2019",
            "sustainability_scheme": "ISCC_EU",
            "max_cfpp_c": "-5",
        },
    )
    db = AsyncMock()
    db.execute.side_effect = [
        _rows([crossing.id]),
        _rows([crossing]),
        _rows([owner]),
        _rows([supplier, buyer]),
    ]

    assessment = await assess_locked_order(db, candidate, organization=supplier)

    assert assessment.indeterminate is False
    assert assessment.would_cross is would_cross
    assert assessment.best_executable_price == (
        crossing.price_per_mt_usd if would_cross else None
    )
