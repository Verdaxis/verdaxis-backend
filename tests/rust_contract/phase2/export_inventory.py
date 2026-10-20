"""Reproduce the source-attested Phase 2 inventory on the test host only.

Run with --write to refresh both fixtures after an approved oracle change.
OpenAPI is requested over the full in-process HTTP app, without startup or DB I/O.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from fastapi.routing import APIRoute
from fastapi.testclient import TestClient

from tests.rust_contract.test_phase2_inventory import GROUP_COUNTS, PLANNED

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
MODULES = {f"app.routers.{name}" for name in GROUP_COUNTS}
EXCLUSIONS = {
    "auth_issuance": "Python-owned: login, refresh, logout, password and session writes",
    "sse": "Python-owned: event stream and outbox dispatcher",
    "alert_triggering": "Python-owned: activity.py price-trigger writer",
    "referral_progression": "Python-owned: signup attribution, VERIFIED and ACTIVE transitions",
}
EFFECTS = {
    "N03": ["notifications.is_read"], "N04": ["notifications.is_read"],
    "P02": ["user_preferences"], "F01": ["feedback_entries"],
    "W02": ["watchlists"], "W03": ["watchlists"],
    "W05": ["watchlist_targets"], "W06": ["watchlist_targets", "watchlist_events"],
    "W08": ["watchlist_events.is_read"],
    "W09": ["watchlist_entries", "watchlists", "watchlist_targets"],
    "W10": ["watchlist_entries", "watchlists", "watchlist_targets", "watchlist_events"],
    "W11": ["watchlists", "watchlist_entries", "watchlist_targets", "watchlist_events"],
    "A01": ["price_alerts"], "A03": ["price_alerts"],
    "R01": ["users.referral_code"], "R04": ["users.referral_code"],
}
UNTYPED_RESPONSES = {
    "N02": {"type": "object", "required": ["count"], "properties": {"count": {"type": "integer"}}},
    "N03": {"type": "object", "required": ["status"], "properties": {"status": {"const": "success"}}},
    "N04": {"type": "object", "required": ["status"], "properties": {"status": {"const": "success"}}},
    "P01": {"type": "object", "additionalProperties": True},
    "P02": {"type": "object", "required": ["value", "updated_at"], "properties": {
        "value": {}, "updated_at": {"type": "string", "format": "date-time"}}},
    "R04": {"type": "object", "required": ["message", "email"], "properties": {
        "message": {"const": "Invitation processed"}, "email": {"type": "string", "format": "email"}}},
}


def selected_route(route):
    return getattr(route.endpoint, "__module__", "") in MODULES


def export_routes(app, openapi):
    """Export all mounted operations owned by the six modules, not a path filter."""
    identities = {(method, path): route_id for route_id, method, path in PLANNED}
    routes = []
    components = openapi["components"]["schemas"]
    schemas = {}

    def collect(value):
        if isinstance(value, dict):
            if "$ref" in value:
                name = value["$ref"].removeprefix("#/components/schemas/")
                if name not in schemas:
                    schemas[name] = components[name]
                    collect(components[name])
            for child in value.values():
                collect(child)
        elif isinstance(value, list):
            for child in value:
                collect(child)

    for mounted in app.routes:
        if not isinstance(mounted, APIRoute) or not selected_route(mounted):
            continue
        for method in sorted(mounted.methods):
            route_id = identities.get((method, mounted.path))
            if route_id is None:
                raise ValueError(f"unreviewed Phase 2 operation: {method} {mounted.path}")
            operation = openapi["paths"][mounted.path][method.lower()]
            success = mounted.status_code or 200
            response = None if success == 204 else operation["responses"][str(success)]["content"]["application/json"]["schema"]
            group = mounted.endpoint.__module__.rsplit(".", 1)[1]
            entry = {
                "id": route_id, "group": group, "method": method, "path": mounted.path,
                "auth": {"R05": "P", "F02": "A"}.get(route_id, "U"),
                "success_status": success,
                "request_schema": {"parameters": operation.get("parameters", []), "body": operation.get("requestBody")},
                "response_schema": UNTYPED_RESPONSES.get(route_id, response),
                "openapi_operation": operation,
                "effect_class": "invite" if route_id == "R04" else "code-allocation" if route_id == "R01" else "read" if method == "GET" else "write",
                "effects": {
                    "allowed_writes": EFFECTS.get(route_id, []),
                    "outbox": "unchanged", "notifications": "unchanged",
                    "email": "attempt-if-new-recipient" if route_id == "R04" else "none",
                    "notification_rows": "is_read-only" if route_id in {"N03", "N04"} else "unchanged",
                },
            }
            collect(entry)
            routes.append(entry)
    # Preference namespace validation happens after arbitrary-JSON HTTP parsing.
    from app.schemas.preferences import NAMESPACE_SCHEMAS
    for namespace, model in NAMESPACE_SCHEMAS.items():
        schemas[f"preference:{namespace}"] = model.model_json_schema()
    order = {route_id: index for index, (route_id, _, _) in enumerate(PLANNED)}
    routes.sort(key=lambda r: order[r["id"]])
    return {"routes": routes, "schemas": schemas}


def write_inventory():
    from app.main import app

    response = TestClient(app, raise_server_exceptions=False).get("/openapi.json")
    response.raise_for_status()
    export = export_routes(app, response.json())
    paths = [
        *(f"app/routers/{g}.py" for g in GROUP_COUNTS),
        *(f"app/schemas/{g}.py" for g in ("preferences", "watchlist", "alerts", "referral")),
        *(f"app/models/{g}.py" for g in ("notification", "user_preference", "feedback", "watchlist", "alerts", "referral", "user")),
        "app/services/watchlists.py", "app/services/watchlist_events.py", "app/services/email.py",
        "app/services/activity.py", "app/services/request_party.py", "app/core/security.py",
        "app/routers/auth_simple.py", "app/main.py", "app/middleware/rbac.py",
        "app/middleware/market_support_scope.py", "app/middleware/preauth_rate_limit.py", "app/rate_limit.py",
        "requirements.txt", "deploy/migration-checkpoints.tsv",
    ]
    document = {
        "version": 1, "oracle_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "sources": {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths},
        "exclusions": EXCLUSIONS, **export,
    }
    inventory = HERE / "routes.json"
    inventory.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n")
    scenarios = []
    for route in export["routes"]:
        cases = ["success", "malformed", "method-slash", "database-error", "cors"]
        if route["auth"] != "P":
            cases += ["auth-matrix", "support-context", "scope", "repeat", "committed-state"]
        for case in cases:
            scenarios.append({"id": f"{route['id']}:{case}", "route_id": route["id"], "case": case})
    special = {
        "notification_scope_and_email_state": "N01", "foreign_watchlist_is_not_found": "W04",
        "pin_and_companion_slice_are_atomic": "W05", "legacy_translation_conflict_preserves_entry": "W09",
        "foreign_alert_delete_is_403": "A03", "invite_failure_preserves_only_pre_send_code_commit": "R04",
        "revocation_waits_for_authenticated_transaction": "N03", "missing_evidence_is_inconclusive": None,
        "auth-token-matrix": None, "auth-negative-cookie-matrix": None, "auth-cutoff-boundaries": None,
        "shared-admin-prefix-rate-limit": "F02", "preference-size-and-validation-order": "P02",
        "feedback-sanitization": "F01", "malformed-stored-pin-window": "W05",
        "watchlist-metrics-visibility": "W04", "pin-snapshot-visibility": "W05",
        "cross-stack-opaque-cursor": "W07", "watchlist-event-postcommit-failure": "W08",
        "radar-postcommit-summary-failure": "W02", "alert-rounding-overflow": "A01",
        "alert-cap-race": "A01", "python-alert-trigger-interop": "A01",
        "referral-code-collisions": "R01", "referral-final-newline": "R05",
        "referral-provider-timeout-duplicate": "R04", "python-referral-progression-interop": "R02",
        "twin-database-seed-identity": None, "whole-user-row-immutability": None,
        "fault-schedules-and-ambiguous-commit": None, "mixed-writer-python-dispatcher": None,
    }
    scenarios += [{"id": name, "route_id": route_id, "case": name} for name, route_id in special.items()]
    disposable = ["responses", "committed-state", "side-effects", "auth", "concurrency", "integration-junit", "artifact-provenance", "cleanup"]
    manifest = {
        "version": 1, "oracle_sha": document["oracle_sha"],
        "inventory_sha256": hashlib.sha256(inventory.read_bytes()).hexdigest(),
        "comparator_profile": "phase2-meaning-v1", "route_ids": [r["id"] for r in export["routes"]],
        "required_scenarios": scenarios, "kyc_switch_states": [False, True],
        "scopes": {"disposable-artifact": disposable, "production-cutover": disposable + ["measurement", "order-write-benchmark", "release-rehearsal", "staging-read-coverage"]},
        "required_identity_fields": ["candidate_sha", "release_binary_sha256", "oracle_sha", "dependency_lock_sha256", "schema_revision", "acl_sha256", "session_time_zone", "seed_sha256", "comparator_profile_sha256", "normalization_sha256", "python_database_identity", "rust_database_identity", "initial_state_sha256"],
        "required_run_fields": ["id", "start", "end", "exit_code", "expected_cases", "observed_cases", "skips", "failures", "input_sha256"],
        "measurement": {"successful_samples_per_route_runtime_size": 1000, "runtimes": ["python", "rust"], "sizes": ["small", "large"], "required_fields": ["workload", "repetitions", "route_weights", "runtime_topology", "sample_counts", "all_statuses", "transport_failures"]},
        "results": "not-run",
    }
    (HERE / "evidence-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", required=True)
    parser.parse_args()
    write_inventory()
