"""Export synthetic reviewed-terms vectors from the Python oracle (Task 6).

`executable_order_terms` binds the full reviewed payload that a direct-hit
review hashes. Rust must reproduce both the canonical payload and its SHA-256
digest byte for byte, so the digest vectors are generated here by the approved
Python helpers and replayed by `rs/read/tests/market_contract.rs`.

These are synthetic vectors, not recorded golden pairs: they exercise escaping,
non-ASCII text, decimal scale, null versus absent, and UTC microsecond text.
They therefore live under `tests/rust_contract/vectors/terms/`, outside the
`golden/<source-sha>/<case-id>/` layout that only recorded responses use.

Regeneration is deterministic; `--check` reports drift. Run this only against
the approved clean source tree.
"""

from __future__ import annotations

import argparse
import json
import sys
from datetime import UTC, date, datetime
from decimal import Decimal
from pathlib import Path
from types import SimpleNamespace
from uuid import UUID

ROOT = Path(__file__).resolve().parents[2]
VECTORS_DIR = ROOT / "tests" / "rust_contract" / "vectors" / "terms"


def _private_lng_ask_terms() -> dict:
    """A private ASK declaration; the two identity fields must never go public."""
    return {
        "side": "ASK",
        "sustainability_scheme": "ISCC_EU",
        "chain_of_custody": "MASS_BALANCE",
        "union_database_registered": True,
        "bio_share_pct": "95.50",
        "feedstock": "MANURE",
        "feedstock_detail": "Slurry from dairy herds",
        "production_country": "NL",
        "wtt_ghg_gco2e_mj": "12.30",
        "specification_standard": "ISO 23306:2020",
        "min_methane_number": "80",
        "delivery_method": "SHIP_TO_SHIP",
        "seller_is_physical_supplier": False,
        "pos_timing": "WITH_DELIVERY",
        "scheme_certificate_number": "ISCC-EU-0001",
        "physical_supplier_name": "Rotterdam Biofuels BV",
    }


def _row(**overrides) -> dict:
    """One order-book row projection as the SQL selects it.

    Values use the wire text Postgres returns: UUID strings, decimal strings
    with their stored scale, RFC 3339 timestamps, ISO dates, and JSON arrays.
    """
    row = {
        "id": "8b1c2d3e-4f50-4a6b-8c7d-9e0f1a2b3c4d",
        "side": "ASK",
        "organization_id": "0f0e0d0c-0b0a-4908-8706-050403020100",
        "owner_user_id": None,
        "created_by_actor_user_id": "11111111-2222-4333-8444-555555555555",
        "creation_method": "MARKET_SUPPORT",
        "support_authorization_id": "99999999-8888-4777-8666-555544443333",
        "provenance": "REAL",
        "product_id": "8f6f34a5-2927-54a6-8564-cca40ae7cb8a",
        "delivery_point_id": "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36",
        "port_id": "NLRTM",
        "vessel_id": None,
        "inventory_item_id": None,
        "quantity_mt": "500.0000",
        "remaining_quantity_mt": "125.5000",
        "price_per_mt_usd": "1180.50",
        "availability_window": "SPOT",
        "delivery_window_start": "2026-11-01",
        "delivery_window_end": None,
        "expires_at": "2026-10-15T12:00:00.123456+00:00",
        "certifications": ["ISCC EU", "REDcert-EU"],
        "certification_declared": True,
        "certification_scheme": "ISCC EU",
        "specification_standard": "ISO 23306:2020",
        "msds_available": True,
        "carbon_intensity_gco2_mj": None,
        "carbon_intensity_method": None,
        "energy_density_mj_kg": "49.10",
        "feedstock": "Manure",
        "origin": "NL",
        "off_spec": False,
        "off_spec_notes": None,
        "is_verdaxis_verified": False,
        "fame_terms": None,
        "lng_terms": _private_lng_ask_terms(),
    }
    row.update(overrides)
    return row


def _namespace(row: dict) -> SimpleNamespace:
    """Present one row projection to `executable_order_terms` as an order."""
    values = dict(row)
    for key in ("quantity_mt", "remaining_quantity_mt", "price_per_mt_usd",
                "carbon_intensity_gco2_mj", "energy_density_mj_kg"):
        values[key] = None if values[key] is None else Decimal(values[key])
    for key in ("id", "organization_id", "owner_user_id",
                "created_by_actor_user_id", "support_authorization_id",
                "product_id", "delivery_point_id", "vessel_id",
                "inventory_item_id"):
        values[key] = None if values[key] is None else UUID(values[key])
    values["expires_at"] = (
        None if values["expires_at"] is None
        else datetime.fromisoformat(values["expires_at"])
    )
    for key in ("delivery_window_start", "delivery_window_end"):
        values[key] = None if values[key] is None else date.fromisoformat(values[key])
    return SimpleNamespace(**values)


def _private_vector() -> dict:
    from app.services.order_terms import (
        executable_order_terms,
        order_terms_digest,
    )

    row = _row()
    order = _namespace(row)
    return {
        "note": "synthetic reviewed terms with a private ASK Bio-LNG declaration",
        "row": row,
        "canonical_terms": executable_order_terms(order),
        "digest": order_terms_digest(order),
        # The private declaration the public projection must redact.
        "lng_terms": row["lng_terms"],
    }


def _non_ascii_vector() -> dict:
    from app.services.order_terms import (
        executable_order_terms,
        order_terms_digest,
    )

    row = _row(
        side="ASK",
        provenance="DEMO",
        creation_method="LEGACY_UNKNOWN",
        support_authorization_id=None,
        created_by_actor_user_id=None,
        certifications=["Ünïcôdé"],
        feedstock="日本語 \"in quotes\" \\ backslash\ttab",
        origin="Ünïcödé 🌱",
        availability_window="2027-Q3",
        off_spec=True,
        off_spec_notes="line one\nline two — em dash",
        carbon_intensity_gco2_mj="0.00",
        carbon_intensity_method="cradle-to-grave",
        fame_terms=None,
        lng_terms=None,
    )
    order = _namespace(row)
    return {
        "note": "non-ASCII and escaping; different JSON encoders must hash the same bytes",
        "row": row,
        "canonical_terms": executable_order_terms(order),
        "digest": order_terms_digest(order),
    }


def _fame_private_terms() -> dict:
    """A complete private FAME ASK declaration with a batch reference."""
    return {
        "schema_version": 1,
        "side": "ASK",
        "neat_fame": True,
        "nomination_status": "IDENTIFIED",
        "uco_mass_pct": 100,
        "standard": "EN_14214",
        "standard_edition": "EN 14214:2019",
        "astm_grade": None,
        "en_climate_class": "B",
        "production_origin": "Singapore",
        "feedstock_origin": "Malaysia",
        "shipping_location": "Jurong",
        "cfpp_c": "-5",
        "cloud_point_c": "-3",
        "ci_gco2e_mj": "18.40",
        "ci_methodology": "ISCC",
        "ci_boundary": "WELL_TO_WAKE",
        "ci_basis": "ACTUAL",
        "lhv_mj_kg": "37.20",
        "sustainability_scheme": "ISCC_EU",
        "certificate_reference": "CERT-0001",
        "certificate_holder": "Jurong Renewables Pte Ltd",
        "certificate_valid_until": "2027-06-30",
        "certificate_scope": None,
        "evidence_status": "AVAILABLE",
        "document_references": ["DOC-1"],
        "batch_reference": "BATCH-9",
        "producing_site": "Jurong Plant",
        "quality_evidence": {
            "status": "AVAILABLE",
            "reference": "Q-1",
            "batch_reference": "BATCH-9",
            "laboratory": "SGS",
            "sampled_on": "2026-09-01",
            "tested_on": "2026-09-03",
            "results": [
                {"property": "WATER_MG_KG", "value": "120", "method": "EN ISO 12937"}
            ],
        },
        "sustainability_evidence": {
            "status": "AVAILABLE",
            "document_type": "POS",
            "reference": "S-1",
            "issuer": "ISCC",
            "quantity_mt": "500.00",
            "supply_date": "2026-10-01",
            "due": "BEFORE_LOADING",
        },
        "evidence_due": "BEFORE_LOADING",
    }


def _fame_vector() -> dict:
    from app.services.order_terms import (
        executable_order_terms,
        order_terms_digest,
    )

    row = _row(
        side="ASK",
        product_id="e561e43f-d9b2-598e-981c-f1d28d515ddc",
        delivery_point_id="73835e92-820e-584b-8280-bb61c63aa28e",
        port_id="SGSIN",
        fame_terms=_fame_private_terms(),
        lng_terms=None,
    )
    order = _namespace(row)
    return {
        "note": "synthetic reviewed terms with a private FAME ASK declaration",
        "row": row,
        "canonical_terms": executable_order_terms(order),
        "digest": order_terms_digest(order),
        # The private declaration the public projection must redact.
        "fame_terms": row["fame_terms"],
    }


def _decimal_validation_vectors() -> list[dict]:
    from copy import deepcopy
    from app.schemas.fame_order import FameAskTerms
    from app.schemas.lng_order import LngAskTerms

    cases = []
    for profile, model, original, field, value in (
        ("lng_v1", LngAskTerms, _private_lng_ask_terms(), "bio_share_pct", "95.500"),
        ("lng_v1", LngAskTerms, _private_lng_ask_terms(), "wtt_ghg_gco2e_mj", "-0.000"),
        ("fame_v1", FameAskTerms, _fame_private_terms(), "ci_gco2e_mj", "18.4000"),
        ("fame_v1", FameAskTerms, _fame_private_terms(), "lhv_mj_kg", "37.2000"),
    ):
        terms = deepcopy(original)
        terms[field] = value
        stored = model.model_validate(terms).model_dump(mode="json")
        cases.append({"profile": profile, "terms": stored, "accepted": True})
    return cases


def _public_terms_vectors() -> list[dict]:
    from copy import deepcopy
    from app.services.fame_order import public_fame_terms
    from app.services.lng_order import public_lng_terms

    cases = []
    for value in ("120.00", "-0.00", "0.00000001"):
        terms = _fame_private_terms()
        terms["quality_evidence"]["results"][0]["value"] = value
        cases.append({"name": f"quality-{value}", "profile": "fame_v1", "terms": terms,
                      "public": public_fame_terms(terms)})
    for profile, terms, project in (
        ("lng_v1", {"side": "BID", "accepted_schemes": ["ISCC_EU"],
                    "accepted_delivery_methods": ["SHIP_TO_SHIP"]}, public_lng_terms),
        ("fame_v1", {"side": "BID", "neat_fame": True, "standard": "EN_14214",
                     "standard_edition": "EN 14214:2019", "sustainability_scheme": "ISCC_EU"}, public_fame_terms),
        ("lng_v1", _private_lng_ask_terms(), public_lng_terms),
        ("fame_v1", _fame_private_terms(), public_fame_terms),
    ):
        cases.append({"name": f"{profile}-{terms['side']}", "profile": profile,
                      "terms": deepcopy(terms), "public": project(terms)})
    pending = _fame_private_terms()
    pending["quality_evidence"] = {"status": "PENDING"}
    cases.append({"name": "pending-quality-defaults", "profile": "fame_v1",
                  "terms": pending, "public": public_fame_terms(pending)})
    for profile in ("lng_v1", "fame_v1"):
        cases.append({"name": f"{profile}-null", "profile": profile, "terms": None, "public": None})
    return cases


def _exact_typed_decimal_vectors() -> list[dict]:
    from copy import deepcopy
    from pydantic import ValidationError
    from app.schemas.fame_order import FameAskTerms, FameBidTerms
    from app.schemas.lng_order import LngAskTerms, LngBidTerms
    from app.services.fame_order import public_fame_terms
    from app.services.lng_order import public_lng_terms

    fame_bid = {"side": "BID", "neat_fame": True, "standard": "EN_14214",
                "standard_edition": "EN 14214:2019", "sustainability_scheme": "ISCC_EU",
                "ci_methodology": "ISCC", "ci_boundary": "WELL_TO_WAKE", "ci_basis": "ACTUAL"}
    lng_bid = {"side": "BID", "accepted_schemes": ["ISCC_EU"],
               "accepted_delivery_methods": ["SHIP_TO_SHIP"]}
    cases = []
    for profile, model, original, fields, project in (
        ("fame_v1", FameAskTerms, _fame_private_terms(), ["cfpp_c", "cloud_point_c"], public_fame_terms),
        ("fame_v1", FameBidTerms, fame_bid, ["max_cfpp_c", "max_cloud_point_c", "max_ci_gco2e_mj"], public_fame_terms),
        ("lng_v1", LngAskTerms, _private_lng_ask_terms(), ["min_methane_number"], public_lng_terms),
        ("lng_v1", LngBidTerms, lng_bid, ["max_wtt_ghg_gco2e_mj", "min_methane_number"], public_lng_terms),
    ):
        for field in fields:
            for value in ("1E-29", "0E-100", "-0.000", "1.000000000000000000000000000001"):
                terms = deepcopy(original)
                terms[field] = value
                stored = model.model_validate(terms).model_dump(mode="json")
                cases.append({"name": f"{profile}-{model.__name__}-{field}-{value}",
                              "profile": profile, "terms": stored, "accepted": True, "public": project(stored)})
    for value in ("1E-29", "0E-100", "-0E-100", "1000000.0000000000000000000000000001", "-1E-29"):
        terms = _fame_private_terms()
        terms["quality_evidence"]["results"][0]["value"] = value
        try:
            stored = FameAskTerms.model_validate(terms).model_dump(mode="json")
        except ValidationError:
            cases.append({"name": f"quality-{value}", "profile": "fame_v1", "terms": terms, "accepted": False})
        else:
            cases.append({"name": f"quality-{value}", "profile": "fame_v1", "terms": stored,
                          "accepted": True, "public": public_fame_terms(stored)})
    for model, profile, original, project, field in (
        (LngAskTerms, "lng_v1", _private_lng_ask_terms(), public_lng_terms, "bio_share_pct"),
        (LngAskTerms, "lng_v1", _private_lng_ask_terms(), public_lng_terms, "wtt_ghg_gco2e_mj"),
        (FameAskTerms, "fame_v1", _fame_private_terms(), public_fame_terms, "ci_gco2e_mj"),
        (FameAskTerms, "fame_v1", _fame_private_terms(), public_fame_terms, "lhv_mj_kg"),
    ):
        for value in ("1.2300", "1.234", "1E-29", "1.230000000000000000000000000001"):
            terms = deepcopy(original)
            terms[field] = value
            try:
                stored = model.model_validate(terms).model_dump(mode="json")
            except ValidationError:
                cases.append({"name": f"{profile}-{field}-{value}", "profile": profile, "terms": terms, "accepted": False})
            else:
                cases.append({"name": f"{profile}-{field}-{value}", "profile": profile, "terms": stored,
                              "accepted": True, "public": project(stored)})
    # Positive lower bound and Python's context-normalized decimal_places check.
    for field, value in (("min_bio_share_pct", "1.000000000000000000000000000001"),
                         ("min_bio_share_pct", "0.999999999999999999999999999999")):
        terms = {**lng_bid, field: value}
        try:
            stored = LngBidTerms.model_validate(terms).model_dump(mode="json")
        except ValidationError:
            cases.append({"name": f"{field}-{value}", "profile": "lng_v1", "terms": terms, "accepted": False})
        else:
            cases.append({"name": f"{field}-{value}", "profile": "lng_v1", "terms": stored,
                          "accepted": True, "public": public_lng_terms(stored)})
    return cases


def _large_decimal_vectors() -> list[dict]:
    from app.services.order_terms import executable_order_terms, order_terms_digest
    cases = []
    for value in ("100000000000000000000000000000", "12345678901234567890123456789",
                  "12345678901234567890123456785", "0.00000000000000000000000000001",
                  "-0.00"):
        row = _row(carbon_intensity_gco2_mj=value)
        order = _namespace(row)
        cases.append({"row": row, "canonical_terms": executable_order_terms(order),
                      "digest": order_terms_digest(order)})
    return cases


def _window_vectors() -> list[dict]:
    from app.services.availability_windows import (
        normalize_availability_window, window_start_date, availability_window_display_label,
    )
    cases = []
    for value in ("２０２６-Q1", "0000-Q1", "0026-Q1", "0026-CAL", "٢٠٢٦-01"):
        try:
            start = window_start_date(value).isoformat()
        except ValueError:
            start = None
        cases.append({"input": value, "normalized": normalize_availability_window(value),
                      "start": start, "label": availability_window_display_label(value)})
    return cases


def _qualification_clock_vectors() -> list[dict]:
    from app.services.fame_order import validate_fame_order_terms
    from app.market_catalog import PRODUCT_IDS
    from fastapi import HTTPException
    from tests.rust_contract.clock import fixed_clock_patches
    cases = []
    for instant in ("2026-10-15T15:59:59+00:00", "2026-10-15T16:00:00+00:00"):
        for quality_date in ("2026-10-15", "2026-10-16"):
            terms = _fame_private_terms()
            terms["certificate_valid_until"] = "2026-10-15"
            terms["quality_evidence"]["sampled_on"] = quality_date
            terms["quality_evidence"]["tested_on"] = quality_date
            now = datetime.fromisoformat(instant)
            with fixed_clock_patches(now):
                try:
                    validate_fame_order_terms(PRODUCT_IDS["UCOME_B100"], "ASK", terms)
                    qualified = True
                except HTTPException:
                    qualified = False
            cases.append({"instant": instant, "terms": terms, "qualified": qualified})
    return cases


def build_vectors() -> dict[str, str]:
    """Return vector file name -> exact committed text."""
    payloads = {
        "private.json": _private_vector(),
        "non_ascii.json": _non_ascii_vector(),
        "fame.json": _fame_vector(),
        "decimal_validation.json": _decimal_validation_vectors(),
        "exact_typed_decimals.json": _exact_typed_decimal_vectors(),
        "public_terms.json": _public_terms_vectors(),
        "large_decimals.json": _large_decimal_vectors(),
        "windows.json": _window_vectors(),
        "qualification_clock.json": _qualification_clock_vectors(),
    }
    return {
        name: json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
        for name, payload in payloads.items()
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="report drift against the committed vectors instead of writing them",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=VECTORS_DIR,
        help="directory to write the vectors to",
    )
    args = parser.parse_args(argv)
    vectors = build_vectors()
    if args.check:
        drift = [
            name
            for name, text in vectors.items()
            if not (args.output / name).exists()
            or (args.output / name).read_text(encoding="utf-8") != text
        ]
        if drift:
            print(f"terms vectors differ: {', '.join(sorted(drift))}", file=sys.stderr)
            return 1
        print(f"terms vectors unchanged: {len(vectors)} files")
        return 0
    args.output.mkdir(parents=True, exist_ok=True)
    for name, text in vectors.items():
        (args.output / name).write_text(text, encoding="utf-8")
    print(f"wrote {len(vectors)} terms vectors to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
