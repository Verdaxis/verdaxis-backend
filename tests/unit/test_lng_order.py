"""Bio-LNG declarations, compatibility and privacy contract."""
from types import SimpleNamespace

import pytest
from fastapi import HTTPException
from pydantic import ValidationError

ASK = {
    "side": "ASK", "sustainability_scheme": "ISCC_EU",
    "scheme_certificate_number": "SELLER-123", "chain_of_custody": "MASS_BALANCE",
    "union_database_registered": True, "bio_share_pct": "100", "feedstock": "MANURE",
    "production_country": "NL", "wtt_ghg_gco2e_mj": "-20.25",
    "specification_standard": "ISO 23306:2020", "min_methane_number": "80",
    "delivery_method": "SHIP_TO_SHIP", "seller_is_physical_supplier": False,
    "physical_supplier_name": "Private bunkering partner", "pos_timing": "WITH_DELIVERY",
}
BID = {
    "side": "BID", "accepted_schemes": ["ISCC_EU"],
    "accepted_delivery_methods": ["SHIP_TO_SHIP"],
}


def test_lng_schema_validation():
    from app.schemas.lng_order import LngAskTerms, LngBidTerms
    assert LngAskTerms.model_validate(ASK).wtt_ghg_gco2e_mj < 0
    assert LngBidTerms.model_validate(BID).require_union_database is True
    for field, value in [
        ("bio_share_pct", "100.01"), ("bio_share_pct", "1.001"),
        ("wtt_ghg_gco2e_mj", "-150.01"), ("wtt_ghg_gco2e_mj", "NaN"),
        ("production_country", "ZZ"), ("production_country", "nl"),
        ("specification_standard", "ISO 8217"), ("physical_supplier_name", " "),
        ("scheme_certificate_number", "x"),
    ]:
        with pytest.raises(ValidationError):
            LngAskTerms.model_validate({**ASK, field: value})
    for field, value in [
        ("accepted_schemes", []), ("accepted_schemes", ["ISCC_EU", "ISCC_EU"]),
        ("accepted_custody", []), ("accepted_delivery_methods", []),
        ("min_bio_share_pct", 0), ("max_wtt_ghg_gco2e_mj", 101),
    ]:
        with pytest.raises(ValidationError):
            LngBidTerms.model_validate({**BID, field: value})


@pytest.mark.parametrize("requirements,declarations,compatible", [
    ({}, {}, True),
    ({"accepted_schemes": ["REDCERT_EU"]}, {}, False),
    ({"accepted_custody": ["PHYSICAL_SEGREGATION"]}, {}, False),
    ({}, {"union_database_registered": False}, False),
    ({"require_union_database": False}, {"union_database_registered": False}, True),
    ({"min_bio_share_pct": "90"}, {"bio_share_pct": "89"}, False),
    ({"min_bio_share_pct": "90"}, {"bio_share_pct": "90"}, True),
    ({"max_wtt_ghg_gco2e_mj": "-30"}, {}, False),
    ({"max_wtt_ghg_gco2e_mj": "-20.25"}, {}, True),
    ({"accepted_delivery_methods": ["TRUCK_TO_SHIP"]}, {}, False),
    ({"min_methane_number": "81"}, {}, False),
    ({"min_methane_number": "80"}, {}, True),
    ({"min_methane_number": "1"}, {"min_methane_number": None}, False),
    ({"accept_resellers": False}, {}, False),
    ({"accept_resellers": False}, {"seller_is_physical_supplier": True}, True),
])
def test_all_compatibility_rules(requirements, declarations, compatible):
    from app.services.lng_order import lng_terms_compatible
    assert lng_terms_compatible({**BID, **requirements}, {**ASK, **declarations}) is compatible


def test_product_side_validation_and_direct_take_error():
    from app.market_catalog import PRODUCT_IDS
    from app.services.lng_order import validate_lng_order_terms, require_lng_counterparty_terms
    product_id = PRODUCT_IDS["BIO_LNG"]
    for product, side, terms in [(product_id, "ASK", None), (product_id, "BID", ASK),
                                  (PRODUCT_IDS["BIO_METHANOL"], "ASK", ASK)]:
        with pytest.raises(HTTPException) as error:
            validate_lng_order_terms(product, side, terms)
        assert error.value.status_code == 422
    order = SimpleNamespace(product_id=product_id, side="ASK", lng_terms=ASK)
    with pytest.raises(HTTPException) as error:
        require_lng_counterparty_terms(order, {**BID, "accept_resellers": False})
    assert error.value.detail == {"code": "LNG_TERMS_INCOMPATIBLE"}


def test_scalar_derivation_and_snapshot_privacy():
    from app.services.lng_order import lng_metadata_fields, lng_trade_snapshot, public_lng_terms, redact_lng_trade_snapshot
    scalars = lng_metadata_fields(ASK)
    assert scalars["carbon_intensity_gco2_mj"] is None
    assert scalars["certification_declared"] is True
    assert scalars["certification_scheme"] == "ISCC EU"
    assert scalars["origin"] == "NL"
    assert scalars["feedstock"] == "Manure"
    public = public_lng_terms(ASK)
    assert "scheme_certificate_number" not in public
    assert "physical_supplier_name" not in public
    assert public["reseller"] is True
    assert public["fueleu_recognition"] == "UNION_DATABASE"
    assert public_lng_terms({**ASK, "union_database_registered": False})["fueleu_recognition"] == "NOT_REGISTERED"
    snapshot = lng_trade_snapshot(BID, ASK)
    assert redact_lng_trade_snapshot(snapshot, reveal_supplier_identity=False)["ask"] == public
    assert redact_lng_trade_snapshot(snapshot, reveal_supplier_identity=True)["ask"]["scheme_certificate_number"] == "SELLER-123"
