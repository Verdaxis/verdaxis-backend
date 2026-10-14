"""Valid Bio-LNG terms shared by unit and PostgreSQL tests."""
from __future__ import annotations


def ask_terms(**overrides) -> dict:
    """A reseller's manure-based ask, registered in the Union Database."""
    terms = {
        "side": "ASK",
        "sustainability_scheme": "ISCC_EU",
        "scheme_certificate_number": "EU-ISCC-Cert-NL201-12345",
        "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": True,
        "bio_share_pct": "100.00",
        "feedstock": "MANURE",
        "feedstock_detail": "Dairy manure digestate",
        "production_country": "NL",
        "wtt_ghg_gco2e_mj": "-45.50",
        "specification_standard": "ISO 23306:2020",
        "min_methane_number": "80",
        "delivery_method": "TRUCK_TO_SHIP",
        "seller_is_physical_supplier": False,
        "physical_supplier_name": "Rotterdam Bunker Partner B.V.",
        "pos_timing": "WITH_DELIVERY",
    }
    terms.update(overrides)
    return terms


def bid_terms(**overrides) -> dict:
    """Requirements that the default ask satisfies."""
    terms = {
        "side": "BID",
        "accepted_schemes": ["ISCC_EU", "REDCERT_EU"],
        "accepted_custody": ["MASS_BALANCE", "PHYSICAL_SEGREGATION"],
        "require_union_database": True,
        "min_bio_share_pct": "90",
        "max_wtt_ghg_gco2e_mj": "20",
        "accepted_delivery_methods": ["TRUCK_TO_SHIP", "SHIP_TO_SHIP"],
        "min_methane_number": "75",
        "accept_resellers": True,
    }
    terms.update(overrides)
    return terms
