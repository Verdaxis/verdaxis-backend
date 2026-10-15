"""Task 6: the committed reviewed-terms vectors are regenerable oracle output.

The digest vectors are the contract between Python's `order_terms` module and
Rust's `market::terms`. These tests prove the committed files are byte-stable,
that each vector's digest really is the Python digest of its canonical payload,
and that the private declarations keep their identity fields so redaction has
something real to remove.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from scripts.rust_contract import export_terms_vectors

ROOT = Path(__file__).resolve().parents[2]
VECTORS_DIR = ROOT / "tests" / "rust_contract" / "vectors" / "terms"

# The two private fields `public_lng_terms` removes from an ASK declaration.
LNG_SECRET_FIELDS = ("scheme_certificate_number", "physical_supplier_name")


def _vector(name: str) -> dict:
    return json.loads((VECTORS_DIR / name).read_text(encoding="utf-8"))


def test_committed_vectors_are_byte_stable():
    regenerated = export_terms_vectors.build_vectors()
    assert sorted(regenerated) == ["fame.json", "non_ascii.json", "private.json"]
    for name, text in regenerated.items():
        assert text == (VECTORS_DIR / name).read_text(encoding="utf-8")


def test_each_digest_is_the_python_digest_of_its_canonical_payload():
    for name in ("private.json", "non_ascii.json", "fame.json"):
        vector = _vector(name)
        encoded = json.dumps(
            vector["canonical_terms"],
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
        assert vector["digest"] == hashlib.sha256(encoded).hexdigest()
        assert len(vector["digest"]) == 64


def test_private_lng_vector_keeps_its_identity_fields():
    vector = _vector("private.json")
    for terms in (vector["row"]["lng_terms"], vector["lng_terms"]):
        for field in LNG_SECRET_FIELDS:
            assert field in terms, "redaction needs a real secret to remove"


def test_private_fame_vector_exposes_its_declaration():
    vector = _vector("fame.json")
    assert vector["fame_terms"] == vector["row"]["fame_terms"]
    assert vector["fame_terms"]["batch_reference"] == "BATCH-9"


def test_non_ascii_vector_is_not_ascii_escaped():
    text = (VECTORS_DIR / "non_ascii.json").read_text(encoding="utf-8")
    assert "Ünïcödé" in text and "\\u00dc" not in text


def test_decimal_scale_and_null_versus_absent_are_exercised():
    private = _vector("private.json")["canonical_terms"]
    assert private["quantity_mt"] == "500"          # "500.0000" normalized
    assert private["remaining_quantity_mt"] == "125.5"
    assert private["delivery_window_end"] is None   # explicit null stays null
    assert "lng_terms" in private
    # The non-ASCII vector omits absent LNG terms entirely, never as null.
    assert "lng_terms" not in _vector("non_ascii.json")["canonical_terms"]
    assert _vector("non_ascii.json")["canonical_terms"]["carbon_intensity_gco2_mj"] == "0"
