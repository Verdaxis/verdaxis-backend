"""Pin the mounted Phase 2 surface, schema exports, and required evidence."""

import hashlib
import json
from collections import Counter
from pathlib import Path

from fastapi.routing import APIRoute
from fastapi.testclient import TestClient

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase2"
GROUP_COUNTS = {
    "notifications": 4, "preferences": 2, "feedback": 2,
    "watchlists": 11, "alerts": 3, "referrals": 5,
}
PLANNED = [
    ("N01", "GET", "/api/notifications"),
    ("N02", "GET", "/api/notifications/unread-count"),
    ("N03", "PATCH", "/api/notifications/{notification_id}/read"),
    ("N04", "PATCH", "/api/notifications/read-all"),
    ("P01", "GET", "/api/users/me/preferences"),
    ("P02", "PUT", "/api/users/me/preferences/{namespace}"),
    ("F01", "POST", "/api/feedback"),
    ("F02", "GET", "/api/admin/feedback"),
    ("W01", "GET", "/api/watchlists"),
    ("W02", "POST", "/api/watchlists/me"),
    ("W03", "POST", "/api/watchlists"),
    ("W04", "GET", "/api/watchlists/{watchlist_id}"),
    ("W05", "POST", "/api/watchlists/{watchlist_id}/targets"),
    ("W06", "DELETE", "/api/watchlists/{watchlist_id}/targets/{target_id}"),
    ("W07", "GET", "/api/watchlists/{watchlist_id}/events"),
    ("W08", "PATCH", "/api/watchlists/{watchlist_id}/events/{event_id}"),
    ("W09", "POST", "/api/watchlists/{watchlist_id}/entries"),
    ("W10", "DELETE", "/api/watchlists/{watchlist_id}/entries/{entry_id}"),
    ("W11", "DELETE", "/api/watchlists/{watchlist_id}"),
    ("A01", "POST", "/api/alerts"),
    ("A02", "GET", "/api/alerts"),
    ("A03", "DELETE", "/api/alerts/{alert_id}"),
    ("R01", "POST", "/api/referrals/my-code"),
    ("R02", "GET", "/api/referrals/my-referrals"),
    ("R03", "GET", "/api/referrals/leaderboard"),
    ("R04", "POST", "/api/referrals/invite"),
    ("R05", "GET", "/api/referrals/resolve/{code}"),
]


def load(name):
    path = FIXTURES / name
    assert path.is_file(), f"missing pinned Phase 2 contract: {name}"
    return json.loads(path.read_text())


def test_exact_route_inventory_and_group_counts():
    routes = load("routes.json")["routes"]
    assert [(r["id"], r["method"], r["path"]) for r in routes] == PLANNED
    assert Counter(r["group"] for r in routes) == GROUP_COUNTS
    assert Counter(r["method"] == "GET" for r in routes) == {True: 11, False: 16}


def test_each_route_pins_contract_and_special_auth_modes():
    routes = load("routes.json")["routes"]
    for route in routes:
        assert route["auth"] == {"R05": "P", "F02": "A"}.get(route["id"], "U")
        assert route["success_status"] in {200, 201, 204}
        assert set(route["request_schema"]) == {"parameters", "body"}
        assert "response_schema" in route
        assert route["effect_class"] in {"read", "write", "code-allocation", "invite"}
        assert route["effects"]["outbox"] == "unchanged"
        assert route["effects"]["notifications"] == "unchanged"
        assert route["effects"]["email"] == ("attempt-if-new-recipient" if route["id"] == "R04" else "none")
        assert route["response_schema"] is None if route["success_status"] == 204 else isinstance(route["response_schema"], dict)


def test_mounted_routes_and_full_http_openapi_export_match():
    from app.main import app
    from tests.rust_contract.phase2.export_inventory import (
        export_routes,
        selected_route,
    )

    document = load("routes.json")
    # No lifespan/startup or database connection is needed to serve OpenAPI.
    response = TestClient(app, raise_server_exceptions=False).get("/openapi.json")
    assert response.status_code == 200
    actual = export_routes(app, response.json())
    assert actual == {k: document[k] for k in ("routes", "schemas")}
    mounted = [
        (method, route.path) for route in app.routes if isinstance(route, APIRoute)
        and selected_route(route) for method in route.methods
    ]
    assert len(mounted) == 27
    assert set(mounted) == {(method, path) for _, method, path in PLANNED}


def test_source_digests_and_exact_exclusions():
    document = load("routes.json")
    for source, digest in document["sources"].items():
        assert hashlib.sha256((ROOT / source).read_bytes()).hexdigest() == digest, source
    assert document["exclusions"] == {
        "auth_issuance": "Python-owned: login, refresh, logout, password and session writes",
        "sse": "Python-owned: event stream and outbox dispatcher",
        "alert_triggering": "Python-owned: activity.py price-trigger writer",
        "referral_progression": "Python-owned: signup attribution, VERIFIED and ACTIVE transitions",
    }
    assert len(document["oracle_sha"]) == 40


def test_manifest_requires_every_route_and_scenario_without_claiming_results():
    document = load("routes.json")
    manifest = load("evidence-manifest.json")
    assert manifest["route_ids"] == [r[0] for r in PLANNED]
    assert manifest["comparator_profile"] == "phase2-meaning-v1"
    assert manifest["inventory_sha256"] == hashlib.sha256((FIXTURES / "routes.json").read_bytes()).hexdigest()
    assert manifest["oracle_sha"] == document["oracle_sha"]
    scenarios = manifest["required_scenarios"]
    ids = [s["id"] for s in scenarios]
    assert len(ids) == len(set(ids))
    for route in document["routes"]:
        required = {"success", "malformed", "method-slash", "database-error"}
        if route["auth"] != "P":
            required |= {"auth-matrix", "support-context", "cors"}
        assert required <= {s["case"] for s in scenarios if s["route_id"] == route["id"]}
    assert {"notification_scope_and_email_state", "foreign_watchlist_is_not_found",
            "pin_and_companion_slice_are_atomic", "legacy_translation_conflict_preserves_entry",
            "foreign_alert_delete_is_403", "invite_failure_preserves_only_pre_send_code_commit",
            "revocation_waits_for_authenticated_transaction", "missing_evidence_is_inconclusive"} <= set(ids)
    assert set(manifest["scopes"]) == {"disposable-artifact", "production-cutover"}
    assert set(manifest["scopes"]["disposable-artifact"]) < set(manifest["scopes"]["production-cutover"])
    assert manifest["measurement"]["successful_samples_per_route_runtime_size"] == 1000
    assert manifest["measurement"]["runtimes"] == ["python", "rust"]
    assert manifest["measurement"]["sizes"] == ["small", "large"]
    assert manifest["kyc_switch_states"] == [False, True]
    assert manifest["results"] == "not-run"
