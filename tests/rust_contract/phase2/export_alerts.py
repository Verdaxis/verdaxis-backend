"""Test-only full-HTTP Pydantic oracle for the Alerts A01-A03 request model.

This export never touches a database. FastAPI validates a request body before the
route handler runs, so the two database dependencies are replaced with synthetic
objects: a current user that already belongs to an organization, and a session
that is never entered. Accepted payloads are reported from the request model
itself, because a valid body would otherwise reach the handler's SQL.

The response wire examples come from a throwaway FastAPI application that
declares `AlertResponse` (and `list[AlertResponse]`) as its response model, so
they are produced by the same serializer the deployed routes use. The Python
trigger helper is out of scope here; `tests/postgres/test_phase2_alert_interop.py`
exercises it against a Rust-created row.

Run `--write` on the isolated test host. `--check` detects stale Python evidence.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from datetime import UTC, datetime
from decimal import Decimal
from pathlib import Path
from types import SimpleNamespace
from uuid import UUID

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "tests/rust_contract/phase2/alerts.json"

# Fixture identities. They are synthetic and carry no runtime authority.
ORGANIZATION_ID = "10000000-0000-4000-8000-000000000001"
PRODUCT_ID = "30000000-0000-4000-8000-000000000001"
DELIVERY_POINT_ID = "40000000-0000-4000-8000-000000000001"
USER_ID = "20000000-0000-4000-8000-000000000001"
ALERT_ID = "50000000-0000-4000-8000-000000000001"
INERT_ALERT_ID = "50000000-0000-4000-8000-000000000002"
CREATED_AT = datetime(2026, 10, 18, 12, 0, 0, tzinfo=UTC)
TRIGGERED_AT = datetime(2026, 10, 18, 12, 30, 0, tzinfo=UTC)

SOURCES = (
    "app/routers/alerts.py",
    "app/schemas/alerts.py",
    "app/models/alerts.py",
)


def _valid_body(**overrides) -> dict:
    body = {
        "product_id": PRODUCT_ID,
        "delivery_point_id": DELIVERY_POINT_ID,
        "direction": "above",
        "threshold_usd": "12.34",
    }
    body.update(overrides)
    return body


def specifications() -> list[dict]:
    """Each vector is `id`, `body`, and the field under test."""
    cases: list[dict] = []

    def add(name: str, body, field: str = "request") -> None:
        cases.append({"id": name, "field": field, "body": body})

    # Accepted payloads: the handler receives these values unchanged.
    add("valid-full", _valid_body())
    add("valid-null-point", _valid_body(delivery_point_id=None))
    add("valid-point-absent", {k: v for k, v in _valid_body().items() if k != "delivery_point_id"})
    add("valid-below", _valid_body(direction="below", threshold_usd="0.01"))
    add("valid-integer-threshold", _valid_body(threshold_usd=1))
    add("valid-integer-string-threshold", _valid_body(threshold_usd="1"))
    add("valid-float-threshold", _valid_body(threshold_usd=1.5))
    add("valid-exponent-threshold", _valid_body(threshold_usd="1e2"))
    add("valid-tiny-threshold", _valid_body(threshold_usd="0.0001"))
    add("valid-extra-field-ignored", _valid_body(is_active=False, org_id=ORGANIZATION_ID))

    # Missing fields.
    add("missing-all-required", {}, field="missing")
    add("missing-product-id", {k: v for k, v in _valid_body().items() if k != "product_id"}, field="product_id")
    add("missing-direction", {k: v for k, v in _valid_body().items() if k != "direction"}, field="direction")
    add("missing-threshold", {k: v for k, v in _valid_body().items() if k != "threshold_usd"}, field="threshold_usd")

    # product_id and delivery_point_id are UUIDs.
    for name, value in (
        ("uuid-invalid", "not-a-uuid"),
        ("uuid-short", "bad"),
        ("uuid-empty", ""),
        ("uuid-null", None),
        ("uuid-number", 123),
        ("uuid-boolean", True),
        ("uuid-array", []),
        ("uuid-object", {}),
        ("uuid-valid-simple-format", PRODUCT_ID.replace("-", "")),
        ("uuid-valid-uppercase", PRODUCT_ID.upper()),
        ("uuid-valid-braces", "{" + PRODUCT_ID + "}"),
    ):
        add("product-id-" + name, _valid_body(product_id=value), field="product_id")
        point_body = _valid_body()
        point_body["delivery_point_id"] = value
        add("delivery-point-id-" + name, point_body, field="delivery_point_id")

    # direction is a plain string with a source validator.
    for name, value in (
        ("invalid-sideways", "sideways"),
        ("invalid-case", "Above"),
        ("invalid-upper", "ABOVE"),
        ("invalid-empty", ""),
        ("invalid-number", 123),
        ("invalid-null", None),
        ("invalid-boolean", True),
        ("invalid-array", []),
        ("invalid-object", {}),
    ):
        add("direction-" + name, _valid_body(direction=value), field="direction")

    # threshold_usd is a Decimal with a source validator.
    for name, value in (
        ("zero", 0),
        ("zero-string", "0"),
        ("zero-decimal-string", "0.00"),
        ("negative-integer", -1),
        ("negative-string", "-0.01"),
        ("parsing-text", "fast"),
        ("parsing-empty", ""),
        ("parsing-null", None),
        ("parsing-boolean", True),
        ("parsing-array", []),
        ("parsing-object", {}),
        ("nan-string", "NaN"),
        ("infinity-string", "Infinity"),
        ("negative-infinity-string", "-Infinity"),
    ):
        add("threshold-" + name, _valid_body(threshold_usd=value), field="threshold_usd")

    # A non-object body fails the request model itself.
    for name, value in (("array", []), ("string", "x"), ("number", 1), ("boolean", True), ("null", None)):
        add("body-" + name, value, field="request")

    # Several failures in one request keep the model's field order.
    add(
        "multiple-errors",
        {"product_id": "bad", "direction": "sideways", "threshold_usd": 0},
        field="request",
    )
    return cases


def _override(app) -> None:
    from app.database import get_db
    from app.routers.auth_simple import get_current_user

    user = SimpleNamespace(
        id=UUID(USER_ID),
        organization_id=UUID(ORGANIZATION_ID),
        email_verified=True,
        status="APPROVED",
        must_change_password=False,
        password_changed_at=None,
        authentication_revoked_at=None,
    )

    def current_user():
        return user

    async def session():
        yield SimpleNamespace()

    app.dependency_overrides[get_current_user] = current_user
    app.dependency_overrides[get_db] = session


def _accepted_values(body) -> dict:
    from app.schemas.alerts import AlertCreate

    parsed = AlertCreate.model_validate(body)
    return {
        "product_id": str(parsed.product_id),
        "delivery_point_id": None if parsed.delivery_point_id is None else str(parsed.delivery_point_id),
        "direction": parsed.direction,
        "threshold_usd": _decimal_text(parsed.threshold_usd),
    }


def _decimal_text(value: Decimal) -> str:
    """The bound parameter text Pydantic hands the driver for a Decimal."""
    return format(value, "f") if value.is_finite() else str(value)


def export() -> dict:
    import fastapi
    import pydantic
    from fastapi import FastAPI
    from fastapi.testclient import TestClient

    from app.main import app

    vectors: list[dict] = []
    overrides = dict(app.dependency_overrides)
    client = TestClient(app, raise_server_exceptions=False)
    try:
        _override(app)
        for spec in specifications():
            body = spec["body"]
            if spec["field"] == "request" and not isinstance(body, dict):
                accepted = False
            else:
                try:
                    _accepted_values(body)
                    accepted = True
                except Exception:
                    accepted = False
            if accepted and isinstance(body, dict):
                vectors.append(
                    {
                        "id": spec["id"],
                        "field": spec["field"],
                        "body": body,
                        "python": {"kind": "accepted", "values": _accepted_values(body)},
                    }
                )
                continue
            if accepted:
                vectors.append(
                    {"id": spec["id"], "field": spec["field"], "body": body,
                     "python": {"kind": "accepted"}}
                )
                continue
            response = client.post(
                "/api/alerts",
                json=body,
                headers={"x-request-id": "phase2-alert-vector"},
            )
            vectors.append(
                {
                    "id": spec["id"],
                    "field": spec["field"],
                    "body": body,
                    "python": {"kind": "rejected", "status": response.status_code,
                               "body": response.text},
                }
            )
    finally:
        app.dependency_overrides.clear()
        app.dependency_overrides.update(overrides)

    # Response wire examples through the production serializer.
    from app.schemas.alerts import AlertResponse

    row = {
        "id": UUID(ALERT_ID),
        "org_id": UUID(ORGANIZATION_ID),
        "product_id": UUID(PRODUCT_ID),
        "product_name": "VLSFO 0.5% Singapore",
        "market_product": "VLSFO_SINGAPORE",
        "delivery_point_id": UUID(DELIVERY_POINT_ID),
        "direction": "above",
        "threshold_usd": Decimal("12.34"),
        "is_active": True,
        "triggered_at": TRIGGERED_AT,
        "created_at": CREATED_AT,
    }
    # The non-nullable identity columns stay real; only the nullable join columns
    # (`product_name`, `market_product`, `delivery_point_id`, `triggered_at`) are
    # absent, which is the shape a row with no matching product produces.
    inert = {
        "id": UUID(INERT_ALERT_ID),
        "org_id": UUID(ORGANIZATION_ID),
        "product_id": UUID(PRODUCT_ID),
        "product_name": None,
        "market_product": None,
        "delivery_point_id": None,
        "direction": "below",
        "threshold_usd": Decimal("0.01"),
        "is_active": False,
        "triggered_at": None,
        "created_at": CREATED_AT,
    }
    witness = FastAPI()

    @witness.get("/one", response_model=AlertResponse)
    async def one() -> AlertResponse:  # pragma: no cover - serializer witness
        return AlertResponse.model_validate(row)

    @witness.get("/many", response_model=list[AlertResponse])
    async def many() -> list[AlertResponse]:  # pragma: no cover - serializer witness
        return [
            AlertResponse.model_validate(row),
            AlertResponse.model_validate(inert),
        ]

    witness_client = TestClient(witness)
    response_examples = {
        "single": witness_client.get("/one").text,
        "list": witness_client.get("/many").text,
    }

    return {
        "version": 1,
        "oracle_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "fixture_only": True,
        "fastapi_version": fastapi.__version__,
        "pydantic_version": pydantic.VERSION,
        "settings": {
            "environment": "test",
            "jwt_secret_env": "JWT_SECRET",
            "jwt_issuer_env": "JWT_ISSUER",
            "jwt_audience_env": "JWT_AUDIENCE",
            "free_tier_alert_limit": 5,
        },
        "sources": {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in SOURCES},
        "python_behaviour": {
            "A01": {
                "success_status": 201,
                "no_organization": {"status": 400, "detail": "User must belong to an organization"},
                "free_tier_reached": {
                    "status": 403,
                    "detail": "Free tier limit of 5 active alerts reached. Delete an existing alert to create a new one.",
                },
                "database_failure": {"status": 500, "detail": "Database operation failed."},
            },
            "A02": {
                "success_status": 200,
                "no_organization": {"status": 400, "detail": "User must belong to an organization"},
                "order": "created_at DESC, no secondary key, inactive rows included",
            },
            "A03": {
                "success_status": 204,
                "no_organization": {"status": 400, "detail": "User must belong to an organization"},
                "not_found": {"status": 404, "detail": "Alert not found"},
                "foreign_organization": {"status": 403, "detail": "You can only delete your own alerts"},
            },
        },
        "admission_order": ["json_syntax_422", "authentication_401", "typed_validation_422", "handler_400_403_404"],
        "response_examples": response_examples,
        "vectors": vectors,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", action="store_true")
    group.add_argument("--check", action="store_true")
    args = parser.parse_args()
    data = export()
    if args.write:
        OUT.write_text(json.dumps(data, indent=2) + "\n")
    else:
        pinned = json.loads(OUT.read_text())
        data["oracle_sha"] = pinned["oracle_sha"]
        if data != pinned:
            raise SystemExit("Python alerts oracle changed; review and regenerate vectors")
    rejected = sum(vector["python"]["kind"] == "rejected" for vector in data["vectors"])
    print(f"ALERT_VECTOR_COUNT={len(data['vectors'])}")
    print(f"ALERT_REJECTED_COUNT={rejected}")


if __name__ == "__main__":
    main()
