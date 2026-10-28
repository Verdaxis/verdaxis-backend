"""Publication declarations retain units, provenance and unknown values."""

from datetime import UTC, date, datetime, timedelta

from fastapi import HTTPException
from pydantic import ValidationError
import pytest

from app.market_catalog import DELIVERY_POINT_IDS, PRODUCT_IDS
from app.schemas.fame import FameListingFuelTerms, FameQualityResult
from app.schemas.supplier_offer import SupplierOfferCreate
from app.services.supplier_offers import validate_offer_dates


@pytest.fixture
def fuel_terms():
    return {
        "neat_fame": True,
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "2012+A2:2019",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "Declared certificate",
        "certificate_holder": "Declared holder",
        "certificate_valid_until": date.today() + timedelta(days=60),
        "evidence_status": "PENDING",
    }


def test_pending_nomination_keeps_unknowns_and_ci_zero_distinct(fuel_terms):
    terms = FameListingFuelTerms.model_validate(fuel_terms)
    assert terms.batch_reference is None
    assert terms.producing_site is None
    assert terms.ci_gco2e_mj is None
    zero_ci = FameListingFuelTerms.model_validate(
        fuel_terms
        | {
            "ci_gco2e_mj": 0,
            "ci_methodology": "Declared methodology",
            "ci_boundary": "Well to tank",
            "ci_basis": "ACTUAL",
        }
    )
    assert zero_ci.ci_gco2e_mj == 0
    with pytest.raises(ValidationError, match="boundary"):
        FameListingFuelTerms.model_validate(
            fuel_terms | {"ci_gco2e_mj": 0, "ci_methodology": "Declared method"}
        )


@pytest.mark.parametrize(
    "grade", ["1-B S15", "1-B S15 LM", "1-B S500", "2-B S15", "2-B S15 LM", "2-B S500"]
)
def test_astm_listings_accept_each_published_grade(fuel_terms, grade):
    terms = FameListingFuelTerms.model_validate(
        fuel_terms
        | {"standard": "ASTM_D6751", "standard_edition": "2024", "astm_grade": grade}
    )
    assert terms.astm_grade == grade


def test_grade_and_nomination_require_their_corresponding_details(fuel_terms):
    with pytest.raises(ValidationError, match="grade"):
        FameListingFuelTerms.model_validate(fuel_terms | {"standard": "ASTM_D6751"})
    with pytest.raises(ValidationError, match="requires ASTM"):
        FameListingFuelTerms.model_validate(fuel_terms | {"astm_grade": "2-B S15"})
    with pytest.raises(ValidationError, match="Identified supply"):
        FameListingFuelTerms.model_validate(
            fuel_terms | {"nomination_status": "IDENTIFIED"}
        )


def test_water_units_are_distinct_and_do_not_apply_standard_pass_limits():
    assert FameQualityResult(property="WATER_MG_KG", value=1500).value == 1500
    assert FameQualityResult(property="WATER_AND_SEDIMENT_VOL_PCT", value=2).value == 2
    with pytest.raises(ValidationError):
        FameQualityResult(property="WATER_AND_SEDIMENT_VOL_PCT", value=101)
    with pytest.raises(ValidationError):
        FameQualityResult(property="WATER_MG_KG", value="NaN")


def test_available_coa_requires_traceability_and_correct_nominated_batch(fuel_terms):
    with pytest.raises(ValidationError, match="reference, batch, laboratory"):
        FameListingFuelTerms.model_validate(
            fuel_terms | {"quality_evidence": {"status": "AVAILABLE"}}
        )
    quality = {
        "status": "AVAILABLE",
        "reference": "CoA reference",
        "batch_reference": "A",
        "laboratory": "Lab",
        "tested_on": date.today(),
    }
    with pytest.raises(ValidationError, match="must match"):
        FameListingFuelTerms.model_validate(
            fuel_terms | {"batch_reference": "B", "quality_evidence": quality}
        )


def test_indicative_offer_can_omit_negotiated_clauses_but_not_future_test_check(
    fuel_terms,
):
    now = datetime.now(UTC)
    payload = SupplierOfferCreate(
        product_id=PRODUCT_IDS["UCOME_B100"],
        delivery_point_id=DELIVERY_POINT_IDS["Singapore"],
        quantity_mt=100,
        min_fill_mt=10,
        price_per_mt_usd=950,
        delivery_basis="EX_TANK",
        named_location="Singapore terminal to be agreed",
        delivery_start=(now + timedelta(days=10)).date(),
        delivery_end=(now + timedelta(days=15)).date(),
        quantity_tolerance_pct=5,
        evidence_due="BEFORE_LOADING",
        expires_at=now + timedelta(days=2),
        fuel_terms=fuel_terms,
    )
    assert payload.payment_terms is None
    validate_offer_dates(payload, now=now)
    future_quality = {
        "status": "PENDING",
        "tested_on": (now + timedelta(days=5)).date(),
    }
    changed = SupplierOfferCreate.model_validate(
        payload.model_dump()
        | {"fuel_terms": fuel_terms | {"quality_evidence": future_quality}}
    )
    with pytest.raises(HTTPException, match="must not be in the future"):
        validate_offer_dates(changed, now=now)
