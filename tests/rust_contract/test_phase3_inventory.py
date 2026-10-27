"""Pin the Phase 3 auth route inventory, client surface and evidence set.

The fixtures under ``tests/rust_contract/phase3/`` are the binding Phase 3
contract. Each test below is a separate control:

* the route inventory and its evidence manifest (section 4.1 and section 10);
* the routes that stay outside Phase 3 and their owning phase (section 4.2);
* the fields, statuses and cookie attributes a client reads (section 7.1);
* the business-rule checklist derived from the Python auth suites (10.2).

Source provenance is recorded as file digests, never as a branch name, so a
moved branch cannot change what was pinned.
"""

from __future__ import annotations

import hashlib
import json
import re
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase3"

# id, group, method, path, auth, origin, cookie effect, endpoint limit,
# pre-auth limit, success status. Values are the binding table in section 4.1.
PHASE3_ROUTES = [
    ("S1", "A", "POST", "/api/auth/login", "P", True, "set-both", 5, 30, 200),
    ("S2", "A", "POST", "/api/auth/refresh", "P", True, "set-both", 10, None, 200),
    ("S3", "A", "POST", "/api/auth/logout", "P", True, "clear-both", None, None, 204),
    ("S4", "A", "GET", "/api/auth/stream-token", "U", False, "none", 30, None, 200),
    ("S5", "A", "PUT", "/api/auth/me/password", "U", True, "set-both", 3, None, 200),
    ("S6", "A", "POST", "/api/auth/invitations/accept", "P", True, "set-both", 5, None, 200),
    ("E1", "B", "POST", "/api/auth/forgot-password", "P", False, "none", 3, 15, 200),
    ("E2", "B", "POST", "/api/auth/reset-password", "P", False, "none", 5, None, 200),
    ("E3", "B", "POST", "/api/auth/verify-email", "P", False, "none", 10, None, 200),
    ("E4", "B", "POST", "/api/auth/resend-verification-email", "P", False, "none", 3, None, 200),
    ("E5", "B", "POST", "/api/auth/register", "P", False, "none", 5, 30, 202),
    ("E6", "B", "POST", "/api/auth/register-with-org", "P", False, "none", 5, 30, 200),
]

GROUP_MEMBERS = {
    "A": ["S1", "S2", "S3", "S4", "S5", "S6"],
    "B": ["E1", "E2", "E3", "E4", "E5", "E6"],
}

# Routes reached through the auth module that stay outside Phase 3 (section 4.2).
PHASE4_AUTH_BUSINESS = [
    ("GET", "/api/auth/me"),
    ("PUT", "/api/auth/me"),
    ("POST", "/api/auth/survey"),
    ("POST", "/api/auth/admin/invitations"),
    ("GET", "/api/auth/admin/invitations/organizations"),
    ("POST", "/api/auth/invitations/resolve"),
    ("PUT", "/api/auth/approve/{user_id}"),
    ("PUT", "/api/auth/reject/{user_id}"),
    ("PUT", "/api/auth/organization/{organization_id}/approve"),
    ("PUT", "/api/auth/organization/{organization_id}/reject"),
    ("GET", "/api/auth/admin/review-queue"),
    ("GET", "/api/auth/admin/review-queue/{user_id}"),
    ("GET", "/api/auth/organization-joins"),
    ("PUT", "/api/auth/organization-joins/{join_request_id}/approve"),
    ("PUT", "/api/auth/organization-joins/{join_request_id}/reject"),
]

# Session-touching routes served by other modules (section 4.2).
OTHER_SESSION_TOUCHING = [
    ("PUT", "/api/admin/analytics/users/{user_id}/reject", 4),
    ("GET", "/api/stream/trades", 6),
    ("GET", "/api/stream/activity", 6),
]

# Error statuses every route must pin (section 4.1; logout clears on any path).
MANDATORY_ERROR_STATUSES = {
    "S1": {401, 403, 422, 429},
    "S2": {401, 409, 429},
    "S3": set(),
    "S4": {403, 429},
    "S5": {400, 422, 429},
    "S6": {400, 422, 429},
    "E1": {429},
    "E2": {400, 422, 429},
    "E3": {400, 429},
    "E4": {429},
    "E5": {422, 429},
    "E6": {400, 409, 422, 429},
}

# Files whose content the inventory was read from. The test recomputes each
# digest, so the pinned contract cannot drift from the source it describes.
SOURCE_FILES = {
    "app/config.py",
    "app/core/security.py",
    "app/main.py",
    "app/middleware/preauth_rate_limit.py",
    "app/rate_limit.py",
    "app/routers/auth_simple.py",
    "app/routing.py",
    "app/schemas/user.py",
}

RACE_NAMES = [
    ("R1", "concurrent_refresh_same_token_yields_one_successor"),
    ("R2", "refresh_replay_after_grace_revokes_family"),
    ("R3", "refresh_vs_logout_revokes_successor"),
    ("R4", "refresh_vs_password_change_never_issues_after_cutoff"),
    ("R5", "refresh_vs_admin_revocation_never_issues_after_cutoff"),
    ("R6", "login_vs_account_status_change_writes_no_session"),
    ("R7", "device_session_limit_is_one_live_family"),
    ("R8", "login_vs_login_different_devices_both_live"),
    ("R9", "refresh_vs_invitation_accept_serializes"),
    ("R10", "login_vs_password_reset_serializes"),
    ("R11", "python_and_rust_device_lock_contend_on_one_key"),
    ("R12", "cutoff_downgrade_fails_fast"),
]

COEXISTENCE_NAMES = [
    "rust_access_token_verifies_in_python_me",
    "python_access_token_verifies_in_rust_notifications",
    "rust_refresh_token_rotates_in_python",
    "python_refresh_token_rotates_in_rust",
    "rust_stream_token_passes_python_stream_check",
    "rust_session_survives_caddy_rollback_to_python",
    "rust_written_refresh_row_is_visible_to_python_cleanup",
    "python_and_rust_device_lock_contend_on_one_key",
]

JOURNEY_IDS = [f"J{index}" for index in range(1, 11)]

PYTHON_AUTH_SUITES = [
    "tests/unit/test_auth.py",
    "tests/unit/test_auth_cookies.py",
    "tests/unit/test_auth_lock_modes.py",
    "tests/unit/test_auth_maintenance.py",
    "tests/unit/test_auth_revocation_contract.py",
    "tests/unit/test_auth_survey.py",
    "tests/unit/test_browser_security_boundaries.py",
    "tests/unit/test_preauth_rate_limit.py",
    "tests/unit/test_security_v2_followup.py",
    "tests/unit/test_security_v2_remediation.py",
    "tests/unit/test_sprint1_security_contracts.py",
    "tests/unit/test_admin_invitations.py",
    "tests/unit/test_refresh_error_contract.py",
    "tests/unit/test_email_verification.py",
    "tests/unit/test_signup_canary_email_suppression.py",
    "tests/unit/test_stream_tokens.py",
    "tests/unit/test_stream_authorization.py",
    "tests/unit/test_admin_review_queue.py",
    "tests/unit/test_organization_join_review.py",
    "tests/unit/test_profile_role_contract.py",
    "tests/postgres/test_auth_read_locks.py",
    "tests/postgres/test_account_rejection_sessions.py",
    "tests/postgres/test_device_session_races.py",
    "tests/postgres/test_refresh_session_concurrency.py",
    "tests/postgres/test_account_approval_email_concurrency.py",
    "tests/postgres/test_product_analytics_login_concurrency.py",
    "tests/postgres/test_organization_market_approval.py",
    "tests/postgres/test_user_status_transitions.py",
    "tests/integration/test_auth_hardening.py",
]

CODE_PATTERN = re.compile(r"^[A-Z][A-Z0-9_]+$")
TEST_NAME_PATTERN = re.compile(r"^[a-z][a-z0-9_]+$")


def load(name):
    path = FIXTURES / name
    assert path.is_file(), f"missing pinned Phase 3 contract: {name}"
    return json.loads(path.read_text())


def route_table():
    return [
        {
            "id": route_id,
            "group": group,
            "method": method,
            "path": path,
            "auth": auth,
            "origin_check": origin,
            "cookie_effect": cookies,
            "endpoint_per_minute": endpoint,
            "preauth_per_minute": preauth,
            "success_status": success,
        }
        for (
            route_id,
            group,
            method,
            path,
            auth,
            origin,
            cookies,
            endpoint,
            preauth,
            success,
        ) in PHASE3_ROUTES
    ]


def _check_evidence_manifest(document):
    """The manifest must bind every route and claim no result."""
    manifest = load("evidence-manifest.json")
    route_ids = [route["id"] for route in document["routes"]]
    assert manifest["route_ids"] == route_ids
    assert manifest["results"] == "not-run"
    assert manifest["inventory"] == "tests/rust_contract/phase3/routes.json"
    assert manifest["checklist"] == "tests/rust_contract/phase3/python-rules-checklist.md"

    scenarios = manifest["required_scenarios"]
    scenario_ids = [scenario["id"] for scenario in scenarios]
    assert len(scenario_ids) == len(set(scenario_ids))
    for scenario in scenarios:
        assert scenario["route_id"] in route_ids
        assert scenario["id"] == f"{scenario['route_id']}:{scenario['case']}"
    for route in document["routes"]:
        cases = {s["case"] for s in scenarios if s["route_id"] == route["id"]}
        required = {"success", "method-slash", "database-error"}
        if route["request"]["encoding"] != "none":
            required.add("validation")
        if (
            route["endpoint_per_minute"] is not None
            or route["preauth_per_minute"] is not None
        ):
            required.add("rate-limit")
        assert required <= cases, route["id"]
        if route["origin_check"]:
            assert "origin" in cases, route["id"]
        if route["cookie_effect"] == "set-both":
            assert "cookies" in cases, route["id"]
        if route["cookie_effect"] == "clear-both":
            assert "cookie-clear-on-error" in cases, route["id"]
        if route["preauth_per_minute"] is not None:
            assert "preauth-limit" in cases, route["id"]
    for route_id in ("E1", "E4", "E5"):
        assert "account-existence" in {
            s["case"] for s in scenarios if s["route_id"] == route_id
        }
    assert "state-machine" in {s["case"] for s in scenarios if s["route_id"] == "S2"}
    for route_id in ("E2", "E3"):
        assert "token-reuse" in {
            s["case"] for s in scenarios if s["route_id"] == route_id
        }

    races = manifest["races"]
    assert [(race["id"], race["name"]) for race in races] == RACE_NAMES
    for race in races:
        assert race["stacks"], race["id"]

    cells = manifest["coexistence"]
    assert [cell["name"] for cell in cells] == COEXISTENCE_NAMES
    assert {cell["direction"] for cell in cells} >= {"rust-to-python", "python-to-rust"}

    assert [journey["id"] for journey in manifest["journeys"]] == JOURNEY_IDS
    assert all(journey["name"] for journey in manifest["journeys"])

    disposable = set(manifest["scopes"]["disposable-candidate"])
    cutover = set(manifest["scopes"]["cutover"])
    assert disposable < cutover
    assert {f"route:{route_id}" for route_id in route_ids} <= disposable
    assert {f"race:{race_id}" for race_id, _ in RACE_NAMES} <= disposable
    assert {f"journey:{journey_id}" for journey_id in JOURNEY_IDS} <= disposable
    assert {f"coexistence:{name}" for name in COEXISTENCE_NAMES} <= cutover
    assert disposable <= cutover

    assert manifest["measurement"] == {"required": False, "reason": "decision 110"}
    assert manifest["security_review"]["required"] is True
    assert manifest["security_review"]["status"] == "not-run"


def test_phase3_route_inventory_is_exact():
    document = load("routes.json")
    routes = document["routes"]

    observed = [
        {
            key: route[key]
            for key in (
                "id",
                "group",
                "method",
                "path",
                "auth",
                "origin_check",
                "cookie_effect",
                "endpoint_per_minute",
                "preauth_per_minute",
                "success_status",
            )
        }
        for route in routes
    ]
    assert observed == route_table()
    assert Counter(route["group"] for route in routes) == {"A": 6, "B": 6}
    assert document["groups"] == {
        "A": {"name": "session core", "route_ids": GROUP_MEMBERS["A"]},
        "B": {"name": "email and registration", "route_ids": GROUP_MEMBERS["B"]},
    }

    for route in routes:
        assert route["path"].startswith("/api/auth/"), route["id"]
        assert route["slash_forms"] == [route["path"], route["path"] + "/"], route["id"]
        assert route["auth"] in {"P", "U"}
        assert route["authenticated_by"], route["id"]
        assert route["request"]["encoding"] in {"none", "form", "json", "query"}
        assert MANDATORY_ERROR_STATUSES[route["id"]] <= set(route["error_statuses"])
        assert route["responses"], route["id"]
        assert route["effects"], route["id"]

    assert document["sources"] == {
        path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest()
        for path in SOURCE_FILES
    }
    assert "branch" not in document
    assert "commit" not in document

    _check_evidence_manifest(document)


def test_phase3_excludes_phase4_business_routes():
    document = load("routes.json")
    phase3 = {(route["method"], route["path"]) for route in document["routes"]}
    assert len(phase3) == 12

    allocations = document["allocations"]
    for entry in allocations:
        assert entry["phase"] in {4, 6}, entry
        assert entry["reason"], entry
        assert (entry["method"], entry["path"]) not in phase3

    auth_business = {
        (entry["method"], entry["path"])
        for entry in allocations
        if entry["module"] == "app.routers.auth_simple"
    }
    assert auth_business == set(PHASE4_AUTH_BUSINESS)
    assert len(auth_business) == 15

    other = {
        (entry["method"], entry["path"]): entry["phase"]
        for entry in allocations
        if entry["module"] != "app.routers.auth_simple"
    }
    assert other == {
        (method, path): phase for method, path, phase in OTHER_SESSION_TOUCHING
    }
    assert Counter(entry["phase"] for entry in allocations) == {4: 16, 6: 2}

    me_limits = {
        entry["method"]: entry["per_client_per_minute"]
        for entry in allocations
        if entry["path"] == "/api/auth/me"
    }
    assert me_limits == {"GET": 120, "PUT": 10}

    # Every mounted auth operation is either in Phase 3 or allocated onward.
    from fastapi.routing import APIRoute

    from app.main import app

    mounted_auth = {
        (method, route.path)
        for route in app.routes
        if isinstance(route, APIRoute)
        and getattr(route.endpoint, "__module__", "") == "app.routers.auth_simple"
        for method in route.methods
    }
    assert len(mounted_auth) == 27
    assert mounted_auth == phase3 | auth_business

    mounted_other = {
        (method, route.path)
        for route in app.routes
        if isinstance(route, APIRoute)
        for method in route.methods
    }
    for method, path, _phase in OTHER_SESSION_TOUCHING:
        assert (method, path) in mounted_other, (method, path)


def test_phase3_client_contract_is_pinned():
    contract = load("client-contract.json")
    inventory = load("routes.json")

    assert contract["error_envelope"] == {
        "shape": {"detail": {"code": "UPPER_SNAKE_CODE", "message": "clear text"}},
        "code_pattern": "^[A-Z][A-Z0-9_]+$",
        "www_authenticate": {"status": 401, "value": "Bearer"},
        "client_tolerates": [
            {"detail": "string"},
            {"detail": [{"msg": "string"}]},
        ],
    }

    assert contract["cookies"] == {
        "names": ["refresh_token", "device_session"],
        "path": "/api/auth",
        "http_only": True,
        "secure": "every environment except development",
        "same_site": "Lax",
        "max_age_seconds": 604800,
        "domain": None,
        "javascript_readable": False,
        "cleared_by": ["POST /api/auth/logout"],
    }

    surfaces = contract["surfaces"]
    assert [surface["id"] for surface in surfaces] == [r["id"] for r in inventory["routes"]]
    for surface, route in zip(surfaces, inventory["routes"]):
        assert surface["method"] == route["method"]
        assert surface["path"] == route["path"]
        assert surface["success"]["status"] == route["success_status"]
        if route["success_status"] != 204:
            assert surface["success"].get("fields") or surface["success"].get("body"), surface["id"]
        for error in surface["errors"]:
            assert error["status"] in route["error_statuses"], surface["id"]
            for code in error["codes"]:
                assert CODE_PATTERN.match(code), (surface["id"], code)

    by_id = {surface["id"]: surface for surface in surfaces}

    login = by_id["S1"]
    assert login["request"] == {
        "encoding": "form",
        "fields": {"username": "string", "password": "string"},
    }
    assert login["success"]["fields"] == {
        "access_token": "string",
        "token_type": {"const": "bearer"},
    }
    assert login["optional_success_fields"] == ["profile"]
    assert login["sets_cookies"] == ["refresh_token", "device_session"]
    login_codes = {
        error["status"]: set(error["codes"]) for error in login["errors"]
    }
    assert login_codes[401] == {"AUTH_CREDENTIALS_INVALID"}
    assert login_codes[403] == {"AUTH_EMAIL_UNVERIFIED", "AUTH_ACCOUNT_NOT_ACTIVE"}
    assert {error["status"] for error in login["errors"] if error.get("www_authenticate")} == {401}

    refresh = by_id["S2"]
    assert refresh["request"] == {"encoding": "none", "fields": {}}
    assert refresh["success"]["fields"] == {
        "access_token": "string",
        "token_type": {"const": "bearer"},
    }
    assert refresh["sets_cookies"] == ["refresh_token", "device_session"]
    terminal = next(error for error in refresh["errors"] if error["status"] == 401)
    assert terminal["terminal"] is True
    assert set(terminal["codes"]) == {
        "REFRESH_TOKEN_MISSING",
        "REFRESH_TOKEN_EXPIRED",
        "REFRESH_TOKEN_INVALID",
        "REFRESH_SESSION_REVOKED",
        "REFRESH_TOKEN_REPLAYED",
        "REFRESH_ACCOUNT_INACTIVE",
        "REFRESH_PASSWORD_CHANGED",
        "REFRESH_DEVICE_REQUIRED",
    }
    rotation = next(error for error in refresh["errors"] if error["status"] == 409)
    assert rotation["codes"] == ["REFRESH_ROTATION_IN_PROGRESS"]
    assert rotation["transient"] is True
    assert rotation["fields"] == {"retry_after_seconds": "integer"}

    logout = by_id["S3"]
    assert logout["success"] == {"status": 204, "fields": {}}
    assert logout["body"] == "ignored by the client"
    assert logout["clears_cookies_on"] == ["success", "error"]

    password = by_id["S5"]
    assert password["request"]["fields"] == {
        "current_password": "string",
        "new_password": "string",
    }
    assert password["success"]["fields"] == {"access_token": "string"}
    assert password["sets_cookies"] == ["refresh_token", "device_session"]
    assert {
        code
        for error in password["errors"]
        if error["status"] == 400
        for code in error["codes"]
    } == {"AUTH_CURRENT_PASSWORD_INVALID"}

    accept = by_id["S6"]
    assert accept["request"]["fields"] == {
        "token": "string",
        "new_password": "string",
        "accept_terms": "boolean",
    }
    assert accept["success"]["fields"] == {
        "access_token": "string",
        "token_type": {"const": "bearer"},
    }
    assert accept["sets_cookies"] == ["refresh_token", "device_session"]

    for route_id in ("E1", "E4"):
        neutral = by_id[route_id]
        assert neutral["success"]["status"] == 200
        assert neutral["success"]["fields"] == {"message": "string"}
        assert neutral["uniform_for_every_valid_address"] is True
        assert neutral["sets_cookies"] == []

    reset = by_id["E2"]
    assert reset["request"]["fields"] == {"token": "string", "new_password": "string"}
    assert reset["success"]["fields"] == {"message": "string"}
    assert {
        code
        for error in reset["errors"]
        if error["status"] == 400
        for code in error["codes"]
    } == {"RESET_TOKEN_INVALID"}

    verify = by_id["E3"]
    assert verify["request"] == {"encoding": "query", "fields": {"token": "string"}}
    assert verify["success"]["fields"] == {
        "message": "string",
        "email": "string",
        "requires_org": "boolean",
    }
    assert verify["optional_success_fields"] == ["registration_token"]
    assert {
        code
        for error in verify["errors"]
        if error["status"] == 400
        for code in error["codes"]
    } == {"EMAIL_TOKEN_INVALID"}

    register = by_id["E5"]
    assert register["success"] == {
        "status": 202,
        "body": {"status": "check_email"},
        "uniform_for_every_valid_body": True,
    }
    assert register["sets_cookies"] == []

    with_org = by_id["E6"]
    assert with_org["request"]["fields"] == {
        "registration_token": "string",
        "organization": "object",
    }
    assert with_org["success"]["fields"] == {"email": "string"}
    assert with_org["success"]["additional_user_fields_allowed"] is True
    assert {
        code
        for error in with_org["errors"]
        if error["status"] == 409
        for code in error["codes"]
    } == {"REGISTRATION_CONFLICT"}

    stream = by_id["S4"]
    assert stream["success"]["fields"] == {"stream_token": "string"}
    assert {
        code
        for error in stream["errors"]
        if error["status"] == 403
        for code in error["codes"]
    } == {"AUTH_ORGANIZATION_REQUIRED"}

    assert contract["refresh_flow"] == {
        "credential": "cookie only",
        "on_401": "one refresh attempt then one retry",
        "keep_token_on_statuses": [500, 502, 503, 504],
        "clear_token_on_other_refusals": True,
        "rotation_in_progress": {
            "status": 409,
            "code": "REFRESH_ROTATION_IN_PROGRESS",
            "outcome": "transient",
            "retries": 1,
        },
    }
    assert contract["cors"] == {
        "origins": "closed per-environment set",
        "credentials": True,
        "wildcard": False,
    }

    assert [change["id"] for change in contract["named_client_changes"]] == [
        "frontend_refresh_treats_rotation_in_progress_as_transient",
        "frontend_login_reads_unverified_code",
        "frontend_registration_is_email_first",
    ]
    assert contract["phase4_allocation"]["limits_per_minute"] == {
        "GET /api/auth/me": 120,
        "PUT /api/auth/me": 10,
    }

    frontend_sources = contract["frontend_sources"]
    assert len(frontend_sources) == 11
    for path, digest in frontend_sources.items():
        assert path.startswith("src/"), path
        assert re.fullmatch(r"[0-9a-f]{64}", digest), path


def test_phase3_python_rule_checklist_is_complete():
    path = FIXTURES / "python-rules-checklist.md"
    assert path.is_file(), "missing pinned Phase 3 rule checklist"

    rows = []
    for line in path.read_text().splitlines():
        if not line.startswith("| `tests/"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        assert len(cells) == 3, line
        suite = cells[0].strip("`")
        rows.append((suite, cells[1], cells[2]))

    assert [suite for suite, _rule, _coverage in rows] == PYTHON_AUTH_SUITES
    for suite, rule, coverage in rows:
        assert (ROOT / suite).is_file(), suite
        assert rule, suite
        assert coverage, suite
        if coverage.startswith("Phase "):
            assert coverage.split()[1] in {"4", "6"}, suite
            continue
        names = re.findall(r"`([^`]+)`", coverage)
        assert names, suite
        for name in names:
            assert TEST_NAME_PATTERN.match(name), (suite, name)

    phase4 = [row for row in rows if row[2].startswith("Phase 4")]
    phase6 = [row for row in rows if row[2].startswith("Phase 6")]
    assert {suite for suite, _rule, _coverage in phase4} == {
        "tests/unit/test_auth_survey.py",
        "tests/unit/test_admin_review_queue.py",
        "tests/unit/test_organization_join_review.py",
        "tests/unit/test_profile_role_contract.py",
        "tests/postgres/test_account_approval_email_concurrency.py",
        "tests/postgres/test_organization_market_approval.py",
    }
    assert {suite for suite, _rule, _coverage in phase6} == {
        "tests/unit/test_stream_authorization.py",
    }
