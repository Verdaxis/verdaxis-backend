"""Export the approved Python catalog constants as attested Rust policy data.

Executable Rust must never branch on product names, codes, or UUIDs. This module
turns ``app.market_catalog`` into committed data artifacts:

* ``deploy/rust/market-policy.json`` - product and delivery-point identity,
  lanes, execution mode, display order, and qualification profile.
* ``deploy/rust/mcp-catalog.json`` - the ``list_markets`` fields that are not
  identity: unit, minimum lot, delivery-point names, and EU scope.

It also writes the pure catalog-projection vectors that
``rs/read/tests/policy.rs`` replays. Regeneration is deterministic, so the
committed files can be byte-compared; ``--check`` reports drift. Run this only
against the approved clean source tree. ``--source-sha`` records the oracle
commit the data was exported from.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
POLICY_PATH = ROOT / "deploy" / "rust" / "market-policy.json"
MCP_PATH = ROOT / "deploy" / "rust" / "mcp-catalog.json"
MANIFEST_PATH = ROOT / "deploy" / "rust" / "market-policy.manifest.json"
VECTORS_DIR = ROOT / "tests" / "rust_contract" / "vectors" / "catalog"
GOLDEN_DIR = ROOT / "tests" / "rust_contract" / "golden"

# Reviewed rule-version mapping, one entry per canonical market product. These
# are profile names (rule versions), not product identities. A catalog change
# that adds or renames a product fails here until a reviewer updates the map.
PROFILES: dict[str, str] = {
    "B30": "scalar",
    "B100": "scalar",
    "UCOME_B100": "fame_v1",
    "BIO_LNG": "lng_v1",
    "BIO_METHANOL": "scalar",
    "E_METHANOL": "scalar",
    "BIO_ETHANOL": "scalar",
    "SYNTHETIC_ETHANOL": "scalar",
}
KNOWN_PROFILES = frozenset({"scalar", "fame_v1", "lng_v1"})
KNOWN_EXECUTION_MODES = frozenset({"ORDERBOOK"})
REQUIRED_PRODUCT_FIELDS = (
    "id",
    "code",
    "name",
    "fuel_type",
    "fuel_grade",
    "execution_mode",
    "lanes",
    "qualification_profile",
    "display_order",
)
# Optional per-product data: only fixed-biofuel products carry it.
OPTIONAL_PRODUCT_FIELDS = ("specification_standard",)
REQUIRED_POINT_FIELDS = ("id", "name", "region", "timezone", "display_order")


class PolicyError(RuntimeError):
    """The policy payload is not valid attested catalog data."""


def recorded_source_sha(root: Path = ROOT) -> str:
    """Return the single recorded oracle corpus SHA under ``golden/``."""
    entries = sorted(path.name for path in (root / "tests" / "rust_contract" / "golden").iterdir())
    entries = [name for name in entries if (root / "tests" / "rust_contract" / "golden" / name).is_dir()]
    if len(entries) != 1:
        raise PolicyError(f"expected exactly one recorded corpus, found {entries}")
    return entries[0]


def build_policy(source_sha: str) -> dict:
    """Build the product/point policy payload from the approved catalog."""
    from app.market_catalog import (
        BIOFUEL_SPECIFICATION_STANDARDS,
        CANONICAL_DELIVERY_POINTS,
        CANONICAL_PRODUCTS,
    )

    codes = {spec.market_product.value for spec in CANONICAL_PRODUCTS}
    if codes != set(PROFILES):
        missing = sorted(codes ^ set(PROFILES))
        raise PolicyError(f"catalog and reviewed profiles disagree on {missing}")
    point_ids = tuple(str(spec.id) for spec in CANONICAL_DELIVERY_POINTS)
    products = [
        {
            "id": str(spec.id),
            "code": spec.market_product.value,
            "name": spec.name,
            "fuel_type": spec.fuel_type,
            "fuel_grade": spec.fuel_grade,
            "execution_mode": spec.execution_mode,
            "lanes": [
                str(point)
                for point in (spec.available_delivery_point_ids or point_ids)
            ],
            "qualification_profile": PROFILES[spec.market_product.value],
            # Fixed-biofuel ASK contracts (BIOFUEL_SPECIFICATION_STANDARDS in
            # app.market_catalog) travel as data so executable SQL and code
            # never name a product. Products without a fixed standard omit the
            # key, matching the Rust loader's skip_serializing_if shape.
            **(
                {"specification_standard": standard}
                if (standard := BIOFUEL_SPECIFICATION_STANDARDS.get(spec.id))
                else {}
            ),
            "display_order": index,
        }
        for index, spec in enumerate(CANONICAL_PRODUCTS, start=1)
    ]
    points = [
        {
            "id": str(spec.id),
            "name": spec.name,
            "region": spec.region,
            "timezone": spec.timezone,
            "display_order": index,
        }
        for index, spec in enumerate(CANONICAL_DELIVERY_POINTS, start=1)
    ]
    payload = {"source_sha": source_sha, "products": products, "points": points}
    validate_policy(payload)
    return payload


def build_mcp_catalog(source_sha: str) -> dict:
    """Build the ``list_markets`` payload from the same approved constants."""
    from app.mcp_server import market_catalog

    dumped = market_catalog().model_dump(mode="json")
    return {"source_sha": source_sha, "products": dumped["products"],
            "delivery_points": dumped["delivery_points"]}


def _valid_source_sha(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 40
        and all(character in "0123456789abcdef" for character in value)
        and set(value) != {"0"}
    )


def validate_policy(payload: object) -> None:
    """Refuse broken identity, lane, or profile data before it is loaded."""
    if not isinstance(payload, dict):
        raise PolicyError("policy must be an object")
    if not _valid_source_sha(payload.get("source_sha")):
        raise PolicyError("policy source SHA is not a full 40-hex commit")
    products = payload.get("products")
    points = payload.get("points")
    if not isinstance(products, list) or not products:
        raise PolicyError("policy must list products")
    if not isinstance(points, list) or not points:
        raise PolicyError("policy must list delivery points")

    point_ids: set[str] = set()
    for point in points:
        if not isinstance(point, dict) or any(field not in point for field in REQUIRED_POINT_FIELDS):
            raise PolicyError("delivery point is missing a required field")
        if point["id"] in point_ids:
            raise PolicyError(f"duplicate delivery point id {point['id']}")
        if not isinstance(point["timezone"], str) or not point["timezone"]:
            raise PolicyError(f"delivery point {point['name']} has no timezone")
        point_ids.add(point["id"])

    codes: set[str] = set()
    product_ids: set[str] = set()
    for product in products:
        if not isinstance(product, dict) or any(
            field not in product for field in REQUIRED_PRODUCT_FIELDS
        ):
            raise PolicyError("product is missing a required field")
        if product["code"] in codes:
            raise PolicyError(f"duplicate product code {product['code']}")
        if product["id"] in product_ids:
            raise PolicyError(f"duplicate product id {product['id']}")
        codes.add(product["code"])
        product_ids.add(product["id"])
        if product["qualification_profile"] not in KNOWN_PROFILES:
            raise PolicyError(f"unknown qualification profile {product['qualification_profile']}")
        if product["execution_mode"] not in KNOWN_EXECUTION_MODES:
            raise PolicyError(f"unknown execution mode {product['execution_mode']}")
        standard = product.get("specification_standard")
        if standard is not None and (
            not isinstance(standard, str) or not standard.strip()
        ):
            raise PolicyError(
                f"product {product['code']} has an invalid specification standard"
            )
        lanes = product["lanes"]
        if not isinstance(lanes, list) or not lanes:
            raise PolicyError(f"product {product['code']} has no lanes")
        for lane in lanes:
            if lane not in point_ids:
                raise PolicyError(f"product {product['code']} references unknown point {lane}")


def render(payload: object) -> str:
    return json.dumps(payload, indent=2, ensure_ascii=False) + "\n"


def _golden_body(root: Path, source_sha: str, case_id: str) -> object:
    path = root / "tests" / "rust_contract" / "golden" / source_sha / case_id / "response.body"
    return json.loads(path.read_text())


def _product_row(item: dict) -> dict:
    return {
        "id": item["id"],
        "name": item["name"],
        "fuel_type": item["fuel_type"],
        "fuel_grade": item["fuel_grade"],
        "unit": item["unit"],
        "min_lot_size": item["min_lot_size"],
        "spec_description": item["spec_description"],
        "is_active": item["is_active"],
    }


def _point_row(item: dict) -> dict:
    return {
        "id": item["id"],
        "name": item["name"],
        "region": item["region"],
        "timezone": item["timezone"],
        "is_active": item["is_active"],
    }


def build_catalog_vectors(root: Path = ROOT) -> dict[str, str]:
    """Replayable projection vectors captured from the recorded catalog routes.

    Each row fixture is the recorded response projected to database columns,
    plus decoys that a strict identity join must drop: a canonical UUID with the
    wrong label, an unknown UUID with the canonical label, and an inactive
    canonical row.
    """
    source_sha = recorded_source_sha(root)
    products_expected = _golden_body(root, source_sha, "R03-products")
    points_expected = _golden_body(root, source_sha, "R04-delivery-points")
    product_rows = [_product_row(item) for item in products_expected]
    point_rows = [_point_row(item) for item in points_expected]
    product_rows += [
        {**_product_row(products_expected[0]), "name": "Legacy Alias"},
        {**_product_row(products_expected[0]), "id": "00000000-0000-4000-8000-00000000ffff"},
        {**_product_row(products_expected[2]), "is_active": False},
    ]
    point_rows += [
        {**_point_row(points_expected[0]), "name": "Alias Point"},
        {**_point_row(points_expected[0]), "id": "00000000-0000-4000-8000-0000000000ff"},
        {**_point_row(points_expected[4]), "is_active": False},
    ]
    return {
        "products_rows.json": render(product_rows),
        "products_expected.json": render(products_expected),
        "points_rows.json": render(point_rows),
        "points_expected.json": render(points_expected),
    }


def build_manifest(policy_text: str, mcp_text: str, source_sha: str) -> dict:
    return {
        "source_sha": source_sha,
        "policy": POLICY_PATH.name,
        "policy_sha256": hashlib.sha256(policy_text.encode()).hexdigest(),
        "mcp_catalog": MCP_PATH.name,
        "mcp_catalog_sha256": hashlib.sha256(mcp_text.encode()).hexdigest(),
    }


def artifacts(root: Path = ROOT, source_sha: str = "") -> dict[str, str]:
    """Return every committed artifact, keyed by its repository-relative path."""
    sha = source_sha or recorded_source_sha(root)
    policy_text = render(build_policy(sha))
    mcp_text = render(build_mcp_catalog(sha))
    files = {
        "deploy/rust/market-policy.json": policy_text,
        "deploy/rust/mcp-catalog.json": mcp_text,
        "deploy/rust/market-policy.manifest.json": render(build_manifest(policy_text, mcp_text, sha)),
    }
    for name, text in build_catalog_vectors(root).items():
        files[f"tests/rust_contract/vectors/catalog/{name}"] = text
    return files


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Export attested catalog policy data")
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--source-sha", default="", help="oracle commit; defaults to the corpus")
    parser.add_argument("--out-dir", type=Path, help="write artifacts here instead of the root")
    parser.add_argument("--check", action="store_true", help="fail when a committed file differs")
    args = parser.parse_args(argv)
    try:
        files = artifacts(args.root, args.source_sha)
    except PolicyError as error:
        print(f"export failed: {error}", file=sys.stderr)
        return 2
    if args.check:
        drift = [
            relative
            for relative, text in files.items()
            if not (args.root / relative).exists() or (args.root / relative).read_text() != text
        ]
        for relative in drift:
            print(f"drift: {relative}")
        return 1 if drift else 0
    for relative, text in files.items():
        path = (args.out_dir or args.root) / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
