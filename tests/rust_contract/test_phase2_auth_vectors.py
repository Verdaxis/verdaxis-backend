"""The token oracle is immutable, source-attested and entirely synthetic."""

import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def test_auth_oracle_inventory_and_source_identity():
    path = ROOT / "tests/rust_contract/phase2/auth-vectors.json"
    document = json.loads(path.read_text())
    assert document["fixture_only"] is True
    assert document["pyjwt_version"] == "2.15.1"
    vectors = document["vectors"]
    ids = [item["id"] for item in vectors]
    assert len(ids) == len(set(ids)) == 161
    assert {"current-key", "previous-key", "previous-key-absent", "wrong-key", "algorithm-none",
            "algorithm-hs384", "exp-before", "exp-equal", "exp-after", "nbf-before", "nbf-equal",
            "nbf-after", "iat-after", "exp-absent", "iat-absent", "nbf-absent", "jti-absent",
            "sub-uppercase", "sub-hex", "sub-braces", "sub-urn", "user-unknown", "user-pending",
            "user-rejected", "password-change-required", "both-cutoffs-password-first",
            "reapproved-old-token", "cookie-only"} <= set(ids)
    assert sum(item["python"]["status"] == 500 for item in vectors) == 4
    for source, digest in document["sources"].items():
        assert hashlib.sha256((ROOT / source).read_bytes()).hexdigest() == digest
    for value in (document["settings"]["current"], document["settings"]["previous"]):
        assert value.startswith("phase2-fixture-")
        assert "not-for-runtime" in value


def test_verifier_has_no_issuing_or_database_interface():
    files = list((ROOT / "rs/read/src/auth").glob("*.rs"))
    assert files
    source = "\n".join(path.read_text() for path in files)
    assert "EncodingKey" not in source
    assert "encode(" not in source
    assert "sqlx::" not in source
    assert "refresh_token" not in source
