"""Test-only full-HTTP preference oracle. Never uses live keys or databases.

The session double answers the two reads the route needs for one request and
rejects writes, so the export captures the route's own validation and response
shape without PostgreSQL. Committed-state, repeat-PUT and concurrency proof
belong to the disposable runner, not this export.

Run `--write` on the isolated test host; `--check` detects stale Python
evidence. The export prints the document between two markers so a lane can
collect the file from a host run whose working tree is not copied back.
"""

import argparse
import copy
import hashlib
import json
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch
from uuid import UUID

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "tests/rust_contract/phase2/preferences.json"
MARKET_POLICY = ROOT / "deploy/rust/market-policy.json"
BEGIN = "PREFERENCES_JSON_BEGIN"
END = "PREFERENCES_JSON_END"

NOW = "2026-10-19T00:00:00+00:00"
USER_ID = "a550c69b-2934-4b36-87f5-e4f9976d1af0"
MAX_BYTES = 8 * 1024
SOURCES = (
    "app/routers/preferences.py",
    "app/schemas/preferences.py",
    "app/models/user_preference.py",
    "app/market_catalog.py",
)

NOTIFICATION_FIELDS = (
    "email_trade_updates",
    "email_market_alerts",
    "email_compliance_digest",
    "email_system_announcements",
    "inapp_trade_updates",
    "inapp_market_alerts",
    "inapp_order_matches",
)


def canonical(value) -> bytes:
    """Python's canonical request encoding, the size unit the route checks."""
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def padded_ascii(total: int, value):
    """A market_watch value whose canonical encoding is exactly `total` bytes."""
    value = copy.deepcopy(value)
    value["portIds"] = [""]
    fixed = len(canonical(value)) - len(canonical(value["portIds"])) + 4
    value["portIds"] = ["a" * (total - fixed)]
    while len(canonical(value)) < total:
        value["portIds"][0] += "a"
    assert len(canonical(value)) == total, (len(canonical(value)), total)
    return value


def padded_multibyte(total: int, value):
    """A market_watch value of exactly `total` bytes built from two-byte glyphs.

    The string has half as many characters as bytes, so a character-count size
    check passes it and a UTF-8 byte check refuses it.
    """
    value = copy.deepcopy(value)
    value["portIds"] = [""]
    fixed = len(canonical(value)) - len(canonical(value["portIds"])) + 4
    # An odd byte budget cannot be filled by two-byte glyphs alone, so spend the
    # single remaining byte on ASCII before the multibyte run.
    count, remainder = divmod(total - fixed, 2)
    value["portIds"] = ["x" * remainder + "é" * count]
    while len(canonical(value)) < total:
        value["portIds"][0] += "é"
    assert len(canonical(value)) == total, (len(canonical(value)), total)
    assert len(value["portIds"][0]) < total, "character count must be below the byte limit"
    return value


def market_watch(products=("B30",), port_ids=()):
    return {"products": list(products), "portIds": list(port_ids)}


def notifications(**changes):
    value = {field: True for field in NOTIFICATION_FIELDS}
    value.update(changes)
    return value


def specifications():
    """Every vector: namespace, the JSON value, and any raw request body.

    `raw` is only set when the request must differ from the canonical encoding,
    for example a whitespace-padded or syntactically invalid body.
    """
    cases = []

    def add(name, namespace, value, **options):
        cases.append({"id": name, "namespace": namespace, "value": value, **options})

    # Valid values for all three namespaces.
    add("market-watch-minimal", "market_watch", market_watch())
    add("market-watch-products-only", "market_watch", market_watch(products=["B100", "B30"]))
    add("market-watch-ports", "market_watch", market_watch(port_ids=["NLRTM", "SGSIN"]))
    add("market-watch-empty-lists", "market_watch", market_watch(products=[], port_ids=[]))
    add("notifications-all-true", "notifications", notifications())
    add("notifications-all-false", "notifications", notifications(**{f: False for f in NOTIFICATION_FIELDS}))
    add("tutorial-true", "tutorial", {"completed": True})
    add("tutorial-false", "tutorial", {"completed": False})

    # Strict scalar types.
    add("tutorial-int", "tutorial", {"completed": 1})
    add("tutorial-float", "tutorial", {"completed": 1.0})
    add("tutorial-string", "tutorial", {"completed": "true"})
    add("tutorial-null", "tutorial", {"completed": None})
    add("tutorial-missing", "tutorial", {})
    add("tutorial-extra", "tutorial", {"completed": True, "zzz": 1})
    add("tutorial-missing-and-extra", "tutorial", {"zzz": 1, "aaa": 2})
    add("notifications-one-int", "notifications", notifications(email_trade_updates=1))
    add("notifications-mixed", "notifications", notifications(**{"email_trade_updates": "true", "inapp_order_matches": None}))
    add("notifications-missing-one", "notifications", notifications())
    add("notifications-partial", "notifications", {"email_trade_updates": False})
    add("notifications-extra", "notifications", {**notifications(), "zzz": True})
    add("market-watch-portid-int", "market_watch", market_watch(port_ids=[1]))
    add("market-watch-portid-bool", "market_watch", market_watch(port_ids=[True]))
    add("market-watch-products-not-list", "market_watch", {"products": "B30", "portIds": []})
    add("market-watch-portids-string", "market_watch", {"products": ["B30"], "portIds": "NLRTM"})

    # Unknown products and every non-object body shape.
    add("market-watch-unknown-product", "market_watch", market_watch(products=["NOPE"]))
    add("market-watch-unknown-product-int", "market_watch", market_watch(products=[5]))
    add("market-watch-unknown-product-bool", "market_watch", market_watch(products=[True]))
    add("market-watch-unknown-product-object", "market_watch", market_watch(products=[{"a": 1}]))
    add("market-watch-mixed-products", "market_watch", market_watch(products=["B30", "NOPE"]))
    add("body-null", "tutorial", None)
    add("body-list", "tutorial", [])
    add("body-string", "tutorial", "true")
    add("body-number", "tutorial", 7)
    add("body-bool", "tutorial", True)
    add("body-empty-object", "tutorial", {})

    # The unknown namespace is refused before the size and schema checks.
    add("unknown-namespace", "legacy", {"completed": True})
    add("unknown-namespace-oversize", "legacy", padded_ascii(MAX_BYTES + 1, market_watch()))
    add("unknown-namespace-null", "legacy", None)
    add("empty-namespace", "", {"completed": True})

    # Canonical UTF-8 size: the limit is inclusive, and the canonical encoding,
    # not the request bytes, is the unit.
    add("size-exact-limit", "market_watch", padded_ascii(MAX_BYTES, market_watch()))
    add("size-over-limit", "market_watch", padded_ascii(MAX_BYTES + 1, market_watch()))
    add("size-multibyte-exact-limit", "market_watch", padded_multibyte(MAX_BYTES, market_watch()))
    add("size-multibyte-over-limit", "market_watch", padded_multibyte(MAX_BYTES + 2, market_watch()))
    small = market_watch(port_ids=["é"])
    add("unicode-raw-and-escaped", "market_watch", small)
    add("whitespace-does-not-count", "market_watch", market_watch(port_ids=["NLRTM"]), raw="  \n\t" + canonical(market_watch(port_ids=["NLRTM"])).decode("utf-8") + "\n ")

    # Syntax is refused before authentication, so it belongs to this route's
    # own contract even though the harness owns the exact parser envelope.
    add("syntax-truncated", "tutorial", None, raw="{")
    add("syntax-trailing", "tutorial", None, raw='{"completed": true,}')
    add("syntax-empty", "tutorial", None, raw="")

    return cases


class Session:
    """One request's database double: read-only rows, no write is performed."""

    def __init__(self, rows):
        self.rows = rows
        self.added = []

    async def execute(self, statement):
        compiled = str(statement)
        if "user_preferences" in compiled and "INSERT" not in compiled.upper():
            text = " ".join(compiled.split())
            rows = self.rows if "namespace" not in text.split("WHERE", 1)[-1] else []
            return SimpleNamespace(
                scalars=lambda: SimpleNamespace(all=lambda: list(rows)),
                scalar_one_or_none=lambda: None,
            )
        return SimpleNamespace(
            scalars=lambda: SimpleNamespace(all=lambda: []),
            scalar_one_or_none=lambda: None,
        )

    def add(self, instance):
        self.added.append(instance)

    async def commit(self):
        pass


def stub_rows():
    return [
        SimpleNamespace(namespace="tutorial", value={"completed": True}),
        SimpleNamespace(namespace="legacy_unknown", value=[1, 2, {"a": None}]),
        SimpleNamespace(namespace="market_watch", value=market_watch(port_ids=["NLRTM"])),
    ]


def export():
    os.environ.setdefault("ENVIRONMENT", "test")
    os.environ.setdefault("DATABASE_URL", "sqlite+aiosqlite:///:memory:")
    os.environ.setdefault("JWT_SECRET", "test-secret-key-for-testing-minimum-32-chars")

    from app.config import settings
    from app.database import get_db
    from app.main import app
    from app.routers.auth_simple import get_current_user
    from app.rate_limit import limiter

    if settings.ENVIRONMENT != "test":
        raise RuntimeError("oracle requires the test environment")

    class Clock:
        @staticmethod
        def now(tz=None):
            from datetime import datetime

            moment = datetime.fromisoformat(NOW)
            return moment.astimezone(tz) if tz else moment.replace(tzinfo=None)

    from fastapi.testclient import TestClient

    user = SimpleNamespace(id=UUID(USER_ID))
    originals = dict(app.dependency_overrides)
    was_enabled = getattr(limiter, "enabled", True)
    # One shared bucket counts per client and route. The export drives many
    # requests from one client, so the limit would hide later vectors.
    limiter.enabled = False
    vectors = []
    try:
        client = TestClient(app, raise_server_exceptions=False)
        app.dependency_overrides[get_current_user] = lambda: user

        async def read_db():
            yield Session(stub_rows())

        app.dependency_overrides[get_db] = read_db
        response = client.get("/api/users/me/preferences")
        read_vector = {
            "id": "get-all-namespaces",
            "method": "GET",
            "namespace": None,
            "value": None,
            "canonical_bytes": None,
            "stored_namespaces": [row.namespace for row in stub_rows()],
            "python_status": response.status_code,
            "python_detail": json.loads(response.text),
        }

        async def write_db():
            yield Session([])

        app.dependency_overrides[get_db] = write_db
        with patch("app.routers.preferences.datetime", Clock):
            for spec in specifications():
                value = spec["value"]
                raw = spec.get("raw")
                if raw is None:
                    raw = canonical(value).decode("utf-8")
                body = raw.encode("utf-8")
                response = client.put(
                    f"/api/users/me/preferences/{spec['namespace']}",
                    content=body,
                    headers={
                        "content-type": "application/json",
                        "x-request-id": "phase2-preferences-vector",
                    },
                )
                detail = json.loads(response.text)
                if response.status_code == 200:
                    # The clock is frozen for the export, so updated_at is
                    # reproducible here; the live route uses the real clock and
                    # the runner compares it against its captured interval.
                    detail["updated_at"] = "<iso8601-aware>"
                vectors.append(
                    {
                        **spec,
                        "raw": raw,
                        "canonical_bytes": None if value is None else len(canonical(value)),
                        "request_bytes": len(body),
                        "python_status": response.status_code,
                        "python_detail": detail,
                    }
                )
    finally:
        limiter.enabled = was_enabled
        app.dependency_overrides.clear()
        app.dependency_overrides.update(originals)

    if any(vector["python_status"] == 429 for vector in vectors):
        raise AssertionError("the export hit the endpoint rate limit; vectors are not evidence")

    policy = json.loads(MARKET_POLICY.read_text())
    return {
        "version": 1,
        "oracle_sha": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip(),
        "fixture_only": True,
        "now": NOW,
        "user_id": USER_ID,
        "max_preference_bytes": MAX_BYTES,
        "read": read_vector,
        "vector_count": len(vectors),
        "sources": {
            path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
            for path in SOURCES
        },
        "policy": {
            "path": "deploy/rust/market-policy.json",
            # The order the Rust port must use for an unknown-product message is
            # the Python enumeration order, which is the route's observed
            # message order, not this artifact's catalog order.
            "artifact_codes": [product["code"] for product in policy["products"]],
        },
        "vectors": vectors,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    data = export()
    if args.write:
        OUT.write_text(json.dumps(data, indent=2) + "\n")
    elif args.check:
        pinned = json.loads(OUT.read_text())
        data["oracle_sha"] = pinned["oracle_sha"]
        if data != pinned:
            raise SystemExit("Python preference oracle changed; review and regenerate vectors")
    else:
        parser.error("choose --write or --check")
    print(f"PREFERENCE_VECTOR_COUNT={len(data['vectors'])}")
    print(f"PREFERENCES_JSON_BEGIN")
    print(json.dumps(data, indent=2))
    print(f"PREFERENCES_JSON_END")


if __name__ == "__main__":
    main()
