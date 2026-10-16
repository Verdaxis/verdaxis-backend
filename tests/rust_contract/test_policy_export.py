"""Task 5: the committed catalog policy is regenerable, attested data.

The exporter turns the approved Python catalog constants into data. These tests
prove the committed artifacts are byte-reproducible, carry the recorded oracle
source SHA, keep the list_markets payload exact, and stay aligned with the
recorded golden catalog responses.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from scripts.rust_contract import export_policy

ROOT = Path(__file__).resolve().parents[2]


def golden_body(case_id: str) -> object:
    sha = export_policy.recorded_source_sha(ROOT)
    path = ROOT / "tests" / "rust_contract" / "golden" / sha / case_id / "response.body"
    return json.loads(path.read_text())


def test_recorded_source_sha_identifies_the_oracle_corpus():
    sha = export_policy.recorded_source_sha(ROOT)
    assert len(sha) == 40
    assert list((ROOT / "tests" / "rust_contract" / "golden").iterdir()) == [
        ROOT / "tests" / "rust_contract" / "golden" / sha
    ]
    payload = json.loads(export_policy.POLICY_PATH.read_text())
    assert payload["source_sha"] == sha


def test_committed_policy_is_byte_stable():
    sha = export_policy.recorded_source_sha(ROOT)
    regenerated = export_policy.render(export_policy.build_policy(sha))
    assert regenerated == export_policy.POLICY_PATH.read_text()


def test_committed_mcp_catalog_is_byte_stable():
    sha = export_policy.recorded_source_sha(ROOT)
    regenerated = export_policy.render(export_policy.build_mcp_catalog(sha))
    assert regenerated == export_policy.MCP_PATH.read_text()


def test_manifest_digests_match_the_committed_artifacts():
    manifest = json.loads(export_policy.MANIFEST_PATH.read_text())
    for field, path in (
        ("policy_sha256", export_policy.POLICY_PATH),
        ("mcp_catalog_sha256", export_policy.MCP_PATH),
    ):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        assert manifest[field] == digest
    assert manifest["source_sha"] == export_policy.recorded_source_sha(ROOT)


def test_mcp_catalog_matches_list_markets():
    from app.mcp_server import market_catalog

    sha = export_policy.recorded_source_sha(ROOT)
    exported = export_policy.build_mcp_catalog(sha)
    assert exported["products"] == market_catalog().model_dump(mode="json")["products"]
    assert exported["delivery_points"] == (
        market_catalog().model_dump(mode="json")["delivery_points"]
    )


def test_policy_covers_every_canonical_identity_exactly():
    sha = export_policy.recorded_source_sha(ROOT)
    payload = export_policy.build_policy(sha)
    from app.market_catalog import CANONICAL_DELIVERY_POINTS, CANONICAL_PRODUCTS

    assert [p["id"] for p in payload["products"]] == [
        str(spec.id) for spec in CANONICAL_PRODUCTS
    ]
    assert [p["display_order"] for p in payload["products"]] == list(
        range(1, len(payload["products"]) + 1)
    )
    assert [p["id"] for p in payload["points"]] == [
        str(spec.id) for spec in CANONICAL_DELIVERY_POINTS
    ]
    assert [p["display_order"] for p in payload["points"]] == list(range(1, 9))


def test_catalog_vectors_match_the_recorded_golden_responses():
    vectors = export_policy.build_catalog_vectors(ROOT)
    assert json.loads(vectors["products_expected.json"]) == golden_body("R03-products")
    assert json.loads(vectors["points_expected.json"]) == golden_body("R04-delivery-points")
    # The row fixtures add decoys; the projection must drop them, so the row
    # count is strictly larger than the expected identity count.
    assert len(json.loads(vectors["products_rows.json"])) > len(
        json.loads(vectors["products_expected.json"])
    )
    assert len(json.loads(vectors["points_rows.json"])) > len(
        json.loads(vectors["points_expected.json"])
    )


def test_regenerated_vectors_are_byte_stable():
    for name, text in export_policy.build_catalog_vectors(ROOT).items():
        assert text == (export_policy.VECTORS_DIR / name).read_text()


def _valid_policy() -> dict:
    return export_policy.build_policy(export_policy.recorded_source_sha(ROOT))


@pytest.mark.parametrize(
    "mutate",
    [
        pytest.param(lambda p: p["products"].append(dict(p["products"][0])), id="duplicate-code"),
        pytest.param(
            lambda p: p["products"].append(
                {**p["products"][0], "code": "FIXTURE_OTHER", "name": "Other"}
            ),
            id="duplicate-uuid",
        ),
        pytest.param(
            lambda p: p["products"][0].__setitem__("qualification_profile", "unknown_rule"),
            id="unknown-profile",
        ),
        pytest.param(lambda p: p["products"][0].__setitem__("lanes", []), id="empty-lanes"),
        pytest.param(
            lambda p: p["products"][0].__setitem__(
                "lanes", ["00000000-0000-4000-8000-00000000dead"]
            ),
            id="unknown-point-reference",
        ),
        pytest.param(
            lambda p: p["points"].append(dict(p["points"][0])), id="duplicate-point"
        ),
        pytest.param(lambda p: p.__setitem__("source_sha", "not-a-sha"), id="bad-source-sha"),
    ],
)
def test_export_validation_refuses_broken_policy(mutate):
    payload = _valid_policy()
    mutate(payload)
    with pytest.raises(export_policy.PolicyError):
        export_policy.validate_policy(payload)


def test_export_validation_accepts_the_committed_policy():
    export_policy.validate_policy(_valid_policy())
