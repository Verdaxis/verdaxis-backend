"""B100 terms must constrain the shared execution path, not imply verification."""

from copy import deepcopy
from datetime import UTC, datetime, timedelta

import pytest
from fastapi import HTTPException

from app.market_catalog import PRODUCT_IDS
from app.models.orderbook import OrderBookOrder, OrderCreationMethod
from app.services.fame_order import (
    fame_terms_compatible,
    fame_trade_snapshot,
    public_fame_terms,
    validate_fame_order_terms,
)


@pytest.fixture
def bid_terms():
    return {
        "side": "BID",
        "neat_fame": True,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
    }


@pytest.fixture
def ask_terms():
    return {
        "side": "ASK",
        "neat_fame": True,
        "uco_mass_pct": 100,
        "evidence_due": "BEFORE_LOADING",
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "private-cert",
        "certificate_holder": "private-holder",
        "certificate_valid_until": str(datetime.now(UTC).date() + timedelta(days=365)),
        "evidence_status": "PENDING",
        "batch_reference": "private-batch",
        "producing_site": "private-site",
    }


def test_fame_entry_requires_correct_side_and_rejects_terms_on_other_fuels(bid_terms):
    with pytest.raises(HTTPException):
        validate_fame_order_terms(PRODUCT_IDS["UCOME_B100"], "BID", None, "SPOT")
    with pytest.raises(HTTPException):
        validate_fame_order_terms(PRODUCT_IDS["UCOME_B100"], "ASK", bid_terms, "SPOT")
    other_product = next(
        value for key, value in PRODUCT_IDS.items() if key != "UCOME_B100"
    )
    with pytest.raises(HTTPException):
        validate_fame_order_terms(other_product, "BID", bid_terms, "SPOT")
    assert validate_fame_order_terms(other_product, "BID", None, "SPOT") is None


@pytest.mark.parametrize(
    "field,value",
    [
        ("standard_edition", "different edition"),
        ("sustainability_scheme", "REDCERT_EU"),
        ("en_climate_class", "Class F"),
    ],
)
def test_matching_requires_requested_standard_and_scheme(
    bid_terms, ask_terms, field, value
):
    assert fame_terms_compatible(bid_terms, ask_terms)
    bid_terms[field] = value
    assert not fame_terms_compatible(bid_terms, ask_terms)


@pytest.mark.parametrize(
    "limit,result",
    [
        ("max_cfpp_c", "cfpp_c"),
        ("max_cloud_point_c", "cloud_point_c"),
    ],
)
def test_cold_flow_limits_require_known_result(bid_terms, ask_terms, limit, result):
    bid_terms[limit] = -5
    assert not fame_terms_compatible(bid_terms, ask_terms)
    ask_terms[result] = -4
    assert not fame_terms_compatible(bid_terms, ask_terms)
    ask_terms[result] = -5
    assert fame_terms_compatible(bid_terms, ask_terms)


def test_ci_zero_is_known_but_method_boundary_basis_must_match(bid_terms, ask_terms):
    bid_terms.update(
        max_ci_gco2e_mj=0, ci_methodology="RED", ci_boundary="WTW", ci_basis="ACTUAL"
    )
    assert not fame_terms_compatible(bid_terms, ask_terms)
    ask_terms.update(
        ci_gco2e_mj=0, ci_methodology="RED", ci_boundary="WTW", ci_basis="ACTUAL"
    )
    assert fame_terms_compatible(bid_terms, ask_terms)
    ask_terms["ci_basis"] = "DEFAULT"
    assert not fame_terms_compatible(bid_terms, ask_terms)


def test_evidence_requirements_do_not_accept_pending_or_wrong_milestone(
    bid_terms, ask_terms
):
    bid_terms["require_sustainability_evidence"] = True
    assert not fame_terms_compatible(bid_terms, ask_terms)
    ask_terms["sustainability_evidence"] = {
        "status": "AVAILABLE",
        "document_type": "POS",
        "reference": "private-pos",
        "due": "BEFORE_DELIVERY",
    }
    assert not fame_terms_compatible(bid_terms, ask_terms)
    ask_terms["sustainability_evidence"]["due"] = "BEFORE_LOADING"
    assert fame_terms_compatible(bid_terms, ask_terms)


def test_expired_certificate_fails_execution_and_public_projection_is_private(
    bid_terms, ask_terms
):
    public = public_fame_terms(ask_terms)
    assert public["side"] == "ASK"
    assert (
        not {
            "batch_reference",
            "producing_site",
            "certificate_reference",
            "certificate_holder",
        }
        & public.keys()
    )
    snapshot = fame_trade_snapshot(bid_terms, ask_terms)
    assert snapshot["ask"]["certificate_reference"] == "private-cert"
    ask_terms["certificate_valid_until"] = str(
        datetime.now(UTC).date() - timedelta(days=1)
    )
    with pytest.raises(HTTPException):
        validate_fame_order_terms(PRODUCT_IDS["UCOME_B100"], "ASK", ask_terms, "SPOT")


def test_trade_snapshot_is_an_immutable_copy(bid_terms, ask_terms):
    snapshot = fame_trade_snapshot(bid_terms, ask_terms)
    original = deepcopy(snapshot)
    ask_terms["certificate_reference"] = "changed"
    assert snapshot == original


@pytest.mark.parametrize(
    "changes",
    [
        {"standard_edition": "x" * 101},
        {
            "ci_gco2e_mj": "10.123",
            "ci_methodology": "RED",
            "ci_boundary": "WTW",
            "ci_basis": "ACTUAL",
        },
        {"lhv_mj_kg": "37.123"},
        {"ci_methodology": "x" * 121},
    ],
)
def test_executable_declarations_fit_legacy_metadata_storage(ask_terms, changes):
    with pytest.raises(HTTPException):
        validate_fame_order_terms(
            PRODUCT_IDS["UCOME_B100"], "ASK", ask_terms | changes, "SPOT"
        )


def test_evidence_commitment_exists_even_before_documents(bid_terms, ask_terms):
    ask_terms["evidence_due"] = "BEFORE_DELIVERY"
    assert not fame_terms_compatible(bid_terms, ask_terms)
    bid_terms["evidence_due"] = "BEFORE_DELIVERY"
    assert fame_terms_compatible(bid_terms, ask_terms)


def test_normal_b100_orders_advance_review_version_without_changing_legacy_orders():
    fame = OrderBookOrder(
        product_id=PRODUCT_IDS["UCOME_B100"],
        creation_method=OrderCreationMethod.SELF_SERVICE,
        version=1,
    )
    fame.bump_version()
    assert fame.version == 2
    other = OrderBookOrder(
        product_id=next(
            value for key, value in PRODUCT_IDS.items() if key != "UCOME_B100"
        ),
        creation_method=OrderCreationMethod.SELF_SERVICE,
        version=1,
    )
    other.bump_version()
    assert other.version == 1
