"""Watchlist route schedules registered with the one shared runner.

The fixture seeds one actor with one container per behavior under test. Python
(the oracle) and Rust read the same cloned state. Where Rust intentionally
differs (PD2, PD3, PD4-G), the action names the rule and a function pins both
observed values and requires every other response field to stay equal.
"""
import hashlib
import json
from datetime import UTC, datetime, timedelta

from scripts.rust_contract.compare import compare_responses
from scripts.rust_contract.phase2_scenarios import (
    RULES,
    Action,
    Scenario,
    ScenarioError,
    Server,
)

ORGANIZATION = "10000000-0000-4000-8000-000000000001"
OWNER = "20000000-0000-4000-8000-000000000001"
REQUEST_ID = "watchlist-read-case"

# Allowlisted deterministic DEMO organization: the only way a disposable run
# may hold a market-visible order without a REAL approval record.
DEMO_ORGANIZATION = "4da7b285-34ee-5443-9406-f96b4ed1a251"
# Same organization, but its KYC state is the one the gate switch decides on.
GATED_USER = "20000000-0000-4000-8000-000000000002"

B30 = "c4cecebc-3d2d-5840-8021-57a9a11bc673"
B100 = "74eb9cb5-f0e3-55f1-b82c-fe29b7c45bf5"
DALIAN = "78281fb3-e726-5396-802e-34b01fda21e8"
BUSAN = "262d36ae-6f35-5785-b9e8-9e221b0f1b78"
SHANGHAI = "633c0593-f9b9-52ad-abe5-a0285f6888d3"
FIXTURE_POINT = "71000000-0000-4000-8000-000000000001"

# Containers. CONTAINER is the PD4-G case kept from the first scenario.
CONTAINER = "70000000-0000-4000-8000-000000000001"
EMPTY = "70000000-0000-4000-8000-000000000002"
LEGACY = "70000000-0000-4000-8000-000000000003"
PUBLIC_PIN = "70000000-0000-4000-8000-000000000004"
HIDDEN_PIN = "70000000-0000-4000-8000-000000000005"
HIDDEN_EVENT = "70000000-0000-4000-8000-000000000006"
GATED_PIN = "70000000-0000-4000-8000-000000000007"
MANY_SLICES = "70000000-0000-4000-8000-000000000008"
FOREIGN = "70000000-0000-4000-8000-000000000009"

# Orders. Each sits in its own slice key so counts do not interact.
PUBLIC_ORDER = "74000000-0000-4000-8000-000000000001"  # public, Dalian
UNKNOWN_ORDER = "74000000-0000-4000-8000-000000000002"  # never public, Busan
GATED_ORDER = "74000000-0000-4000-8000-000000000003"  # public only without the KYC gate, Shanghai

# Wrong length, a bad character in the simple form, and one in the hyphenated form.
MALFORMED_UUIDS = (
    ("malformed-uuid-length", "not-a-uuid"),
    ("malformed-uuid-simple", "0123456789abcdef0123456789abcdeg"),
    ("malformed-uuid-hyphenated", "01234567-89ab-cdef-0123-456789abcdeg"),
)

BASE_TIME = datetime(2026, 10, 19, tzinfo=UTC)
FAR_FUTURE = datetime(2100, 1, 1, tzinfo=UTC)


def _at(days=0, minutes=0):
    return BASE_TIME + timedelta(days=days, minutes=minutes)


def _container(connection, container_id, user_id, name, created_at):
    connection.execute(
        "INSERT INTO watchlists(id,user_id,name,kind,created_at) VALUES(%s,%s,%s,'CUSTOM',%s)",
        (container_id, user_id, name, created_at))


def _slice(connection, target_id, container_id, product_code, point_id, window, created_at):
    connection.execute(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,"
        "delivery_point_id,availability_window_code,created_at) VALUES(%s,%s,'SLICE',%s,%s,%s,%s)",
        (target_id, container_id, product_code, point_id, window, created_at))


def _pin(connection, target_id, container_id, order_id, point_id, created_at):
    connection.execute(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,"
        "delivery_point_id,availability_window_code,order_id,snapshot_price_per_mt_usd,"
        "snapshot_quantity_mt,snapshot_remaining_quantity_mt,snapshot_status,snapshot_side,"
        "snapshot_market_product,snapshot_availability_window,created_at) "
        "VALUES(%s,%s,'PIN','B30',%s,'SPOT',%s,612.5,100,100,'OPEN','BID','B30','SPOT',%s)",
        (target_id, container_id, point_id, order_id, created_at))


def _event(connection, event_id, container_id, target_id, event_type, payload, created_at, is_read=False):
    connection.execute(
        "INSERT INTO watchlist_events(id,watchlist_id,watchlist_target_id,event_type,"
        "event_payload,is_read,created_at) VALUES(%s,%s,%s,%s,%s::json,%s,%s)",
        (event_id, container_id, target_id, event_type, json.dumps(payload), is_read, created_at))


def _order(connection, order_id, organization_id, owner_id, provenance, point_id, expires_at):
    connection.execute(
        "INSERT INTO orderbook_orders(id,organization_id,owner_user_id,provenance,side,product_id,"
        "delivery_point_id,quantity_mt,remaining_quantity_mt,price_per_mt_usd,availability_window,"
        "status,expires_at,created_at,updated_at) "
        "VALUES(%s,%s,%s,%s,'BID',%s,%s,100,100,600,'SPOT','OPEN',%s,%s,%s)",
        (order_id, organization_id, owner_id, provenance, B30, point_id, expires_at, BASE_TIME, BASE_TIME))


def _seed_users_and_orders(connection):
    connection.execute(
        "INSERT INTO organizations(id,name,domain,type,verification_status,provenance) "
        "VALUES(%s,'Phase 2 demo fixture','demo.phase2.invalid','FUEL_BUYER','APPROVED','DEMO')",
        (DEMO_ORGANIZATION,))
    # PENDING KYC: not eligible with the gate on, eligible with the gate off.
    connection.execute(
        "INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,"
        "must_change_password) VALUES(%s,'gated@phase2.invalid','not-a-login','BUYER','APPROVED',%s,true,false)",
        (GATED_USER, DEMO_ORGANIZATION))
    _order(connection, PUBLIC_ORDER, DEMO_ORGANIZATION, None, "DEMO", DALIAN, FAR_FUTURE)
    _order(connection, UNKNOWN_ORDER, ORGANIZATION, None, "UNKNOWN", BUSAN, None)
    _order(connection, GATED_ORDER, DEMO_ORGANIZATION, GATED_USER, "DEMO", SHANGHAI, FAR_FUTURE)


def _seed_best_price_events(connection):
    """PD4-G: three best-price rows before, at and after any cutoff."""
    _container(connection, CONTAINER, OWNER, "Read fixture", BASE_TIME)
    connection.execute(
        "INSERT INTO delivery_points(id,name,region,is_active) VALUES(%s,'Read fixture point','Fixture',true)",
        (FIXTURE_POINT,))
    target = "72000000-0000-4000-8000-000000000001"
    _slice(connection, target, CONTAINER, "TEST_PRODUCT", FIXTURE_POINT, "SPOT", BASE_TIME)
    for index, offset in enumerate((-1, 0, 1), start=1):
        _event(connection, f"73000000-0000-4000-8000-{index:012d}", CONTAINER, target,
               "SLICE_BEST_PRICE_MOVED", {}, _at(days=offset))


def _seed_legacy_entries(connection):
    _container(connection, EMPTY, OWNER, "Empty", _at(minutes=1))
    _container(connection, LEGACY, OWNER, "Legacy entries", _at(minutes=2))
    for index, (product, point) in enumerate(((B30, DALIAN), (B100, None)), start=1):
        connection.execute(
            "INSERT INTO watchlist_entries(id,watchlist_id,product_id,delivery_point_id,created_at) "
            "VALUES(%s,%s,%s,%s,%s)",
            (f"75000000-0000-4000-8000-{index:012d}", LEGACY, product, point, _at(minutes=index)))


def _order_event(order_id):
    return {"source_kind": "LIVE_ORDER", "demo_status": "DEMO", "scope": "DELIVERY_POINT",
            "order_id": order_id, "old_price_per_mt_usd": 590.0, "new_price_per_mt_usd": 612.5}


def _seed_pin_cases(connection):
    # Public slice and PIN: Python and Rust agree on every field.
    _container(connection, PUBLIC_PIN, OWNER, "Public slice and pin", _at(minutes=3))
    _slice(connection, "72000000-0000-4000-8000-000000000010", PUBLIC_PIN, "B30", DALIAN, "SPOT", _at(minutes=3))
    _pin(connection, "72000000-0000-4000-8000-000000000011", PUBLIC_PIN, PUBLIC_ORDER, DALIAN, _at(minutes=4))
    _event(connection, "73000000-0000-4000-8000-000000000101", PUBLIC_PIN,
           "72000000-0000-4000-8000-000000000011", "PIN_PRICE_CHANGED", _order_event(PUBLIC_ORDER), _at(days=1))
    _event(connection, "73000000-0000-4000-8000-000000000102", PUBLIC_PIN,
           "72000000-0000-4000-8000-000000000010", "SLICE_NEW_ORDER", _order_event(PUBLIC_ORDER),
           _at(days=2), is_read=True)

    # PIN and events whose source order is never public (PD2, PD3).
    _container(connection, HIDDEN_PIN, OWNER, "Hidden order pin", _at(minutes=5))
    _slice(connection, "72000000-0000-4000-8000-000000000020", HIDDEN_PIN, "B30", BUSAN, "SPOT", _at(minutes=5))
    _pin(connection, "72000000-0000-4000-8000-000000000021", HIDDEN_PIN, UNKNOWN_ORDER, BUSAN, _at(minutes=6))
    _event(connection, "73000000-0000-4000-8000-000000000201", HIDDEN_PIN,
           "72000000-0000-4000-8000-000000000021", "PIN_PRICE_CHANGED", _order_event(UNKNOWN_ORDER), _at(days=1))
    _event(connection, "73000000-0000-4000-8000-000000000202", HIDDEN_PIN,
           "72000000-0000-4000-8000-000000000020", "SLICE_NEW_ORDER", _order_event(UNKNOWN_ORDER), _at(days=2))

    # Visible PIN, but a slice event names a hidden order (PD2 only: no count change).
    _container(connection, HIDDEN_EVENT, OWNER, "Hidden order event", _at(minutes=7))
    _slice(connection, "72000000-0000-4000-8000-000000000030", HIDDEN_EVENT, "B30", DALIAN, "SPOT", _at(minutes=7))
    _pin(connection, "72000000-0000-4000-8000-000000000031", HIDDEN_EVENT, PUBLIC_ORDER, DALIAN, _at(minutes=8))
    _event(connection, "73000000-0000-4000-8000-000000000301", HIDDEN_EVENT,
           "72000000-0000-4000-8000-000000000031", "PIN_PRICE_CHANGED", _order_event(PUBLIC_ORDER), _at(days=1))
    _event(connection, "73000000-0000-4000-8000-000000000302", HIDDEN_EVENT,
           "72000000-0000-4000-8000-000000000030", "SLICE_NEW_ORDER", _order_event(UNKNOWN_ORDER), _at(days=2))

    # Source order public only while the KYC execution gate is off.
    _container(connection, GATED_PIN, OWNER, "Gate dependent order", _at(minutes=9))
    _slice(connection, "72000000-0000-4000-8000-000000000040", GATED_PIN, "B30", SHANGHAI, "SPOT", _at(minutes=9))
    _pin(connection, "72000000-0000-4000-8000-000000000041", GATED_PIN, GATED_ORDER, SHANGHAI, _at(minutes=10))
    _event(connection, "73000000-0000-4000-8000-000000000401", GATED_PIN,
           "72000000-0000-4000-8000-000000000041", "PIN_PRICE_CHANGED", _order_event(GATED_ORDER), _at(days=1))
    _event(connection, "73000000-0000-4000-8000-000000000402", GATED_PIN,
           "72000000-0000-4000-8000-000000000040", "SLICE_NEW_ORDER", _order_event(GATED_ORDER), _at(days=2))


def _seed_many_slices(connection):
    """101 slices with distinct creation times: totals count before truncation."""
    _container(connection, MANY_SLICES, OWNER, "Many slices", _at(minutes=11))
    connection.execute(
        "INSERT INTO watchlist_targets(id,watchlist_id,target_type,market_product_code,delivery_point_id,"
        "availability_window_code,created_at) "
        "SELECT ('76000000-0000-4000-8000-'||lpad(n::text,12,'0'))::uuid,%s,'SLICE','B30',%s,"
        "to_char(date '2026-01-01'+(n-1)*interval '1 month','YYYY-MM'),%s::timestamptz+n*interval '1 minute' "
        "FROM generate_series(1,101) n",
        (MANY_SLICES, DALIAN, _at(minutes=11)))


def _seed_foreign_container(connection):
    _container(connection, FOREIGN, GATED_USER, "Foreign container", _at(minutes=12))


def _setup(connection):
    _seed_users_and_orders(connection)
    _seed_best_price_events(connection)
    _seed_legacy_entries(connection)
    _seed_pin_cases(connection)
    _seed_many_slices(connection)
    _seed_foreign_container(connection)


# Observation: counts and presence only. Timestamps stay in the strict
# comparison of the remaining response fields.
def _observe(body):
    return {
        "unread": body["unread_event_count"],
        "latest": body["latest_event_at"] is not None,
        "slices": [
            {"active": item["active_order_count"], "unread": item["unread_event_count"],
             "latest": item["latest_event_at"] is not None,
             "pins": [{"unread": pin["unread_event_count"]} for pin in item["pins"]]}
            for item in body["slices"]],
    }


def _hide_all(body):
    """Rust drops every best-price row, hidden PIN, hidden-order event and count."""
    body["unread_event_count"] = 0
    body["latest_event_at"] = None
    for item in body["slices"]:
        item["active_order_count"] = 0
        item["unread_event_count"] = 0
        item["latest_event_at"] = None
        item["pins"] = []


def _hide_source_event_only(body):
    """Only the hidden-order slice event disappears; the visible PIN event stays."""
    item = body["slices"][0]
    latest = item["pins"][0]["latest_event_at"]
    body["unread_event_count"] = item["unread_event_count"] = 1
    body["latest_event_at"] = item["latest_event_at"] = latest


def _pair_difference(route_id, python_observed, rust_observed, adapt):
    """Pin both sides' observations, then compare the adapted pair strictly."""
    def check(responses):
        left, right = (json.loads(response["body"]) for response in responses)
        if _observe(left) != python_observed:
            raise ScenarioError(f"Python observation differs from approved pair: {_observe(left)}")
        if _observe(right) != rust_observed:
            raise ScenarioError(f"Rust observation differs from approved pair: {_observe(right)}")
        adapt(left)
        wire = [{key: response[key] for key in ("status", "headers", "body")} for response in responses]
        wire[0]["body"] = json.dumps(left).encode()
        # The adapted body changed length; keep the declared length truthful.
        wire[0]["headers"] = {
            key: str(len(wire[0]["body"])) if key.lower() == "content-length" else value
            for key, value in wire[0]["headers"].items()}
        problems = compare_responses(
            wire[0], wire[1], profile="phase2-meaning-v1", route_id=route_id,
            rules_sha256=hashlib.sha256(RULES.read_bytes()).hexdigest(), request_id=REQUEST_ID)
        if problems:
            raise ScenarioError(f"unapproved watchlist field difference: {problems}")
    return check


NOTHING_VISIBLE = {"unread": 0, "latest": False, "slices": [
    {"active": 0, "unread": 0, "latest": False, "pins": []}]}


def _unchanged(before, after, response, mail):
    if before != after or mail:
        raise ScenarioError("watchlist read changed state or sent email")


def _detail(name, container_id, headers, *, difference=None, python=None, rust=None, adapt=None):
    options = {}
    if difference:
        options = {"approved_difference": difference,
                   "assert_http_difference": _pair_difference("W04", python, rust, adapt)}
    return Action(name, "W04", "GET", "/api/watchlists/" + container_id, 200,
                  headers=headers, assert_after=_unchanged, **options)


def _gate_dependent(headers, gate_enabled):
    """With the gate on the order is hidden (PD2, PD3); with it off both agree."""
    if not gate_enabled:
        return _detail("gate-off-order-visible", GATED_PIN, headers)
    return _detail(
        "gate-on-order-hidden", GATED_PIN, headers, difference="PD2+PD3",
        python={"unread": 2, "latest": True, "slices": [
            {"active": 1, "unread": 2, "latest": True, "pins": [{"unread": 1}]}]},
        rust=NOTHING_VISIBLE, adapt=_hide_all)


def _actions(context, gate_enabled):
    import jwt
    now = datetime.now(UTC)
    token = jwt.encode(
        {"sub": OWNER, "type": "access", "iss": "verdaxis-test-api", "aud": "verdaxis-test-web",
         "iat": int(now.timestamp()), "exp": int((now + timedelta(minutes=30)).timestamp())},
        context.environment["JWT_SECRET"], algorithm="HS256")
    headers = {"authorization": "Bearer " + token, "x-request-id": REQUEST_ID}
    return [
        Action("legacy-list", "W01", "GET", "/api/watchlists", 200, headers=headers, assert_after=_unchanged),
        _detail("owner-empty", EMPTY, headers),
        _detail("legacy-entries-detail", LEGACY, headers),
        _detail("best-price-events-hidden", CONTAINER, headers, difference="PD4-G",
                python={"unread": 3, "latest": True, "slices": [
                    {"active": 0, "unread": 3, "latest": True, "pins": []}]},
                rust=NOTHING_VISIBLE, adapt=_hide_all),
        _detail("public-slice-and-pin", PUBLIC_PIN, headers),
        _detail("hidden-order-pin-and-event", HIDDEN_PIN, headers, difference="PD2+PD3",
                python={"unread": 2, "latest": True, "slices": [
                    {"active": 1, "unread": 2, "latest": True, "pins": [{"unread": 1}]}]},
                rust=NOTHING_VISIBLE, adapt=_hide_all),
        _detail("hidden-order-event-only", HIDDEN_EVENT, headers, difference="PD2",
                python={"unread": 2, "latest": True, "slices": [
                    {"active": 1, "unread": 2, "latest": True, "pins": [{"unread": 1}]}]},
                rust={"unread": 1, "latest": True, "slices": [
                    {"active": 1, "unread": 1, "latest": True, "pins": [{"unread": 1}]}]},
                adapt=_hide_source_event_only),
        _gate_dependent(headers, gate_enabled),
        _detail("totals-before-truncation", MANY_SLICES, headers),
        Action("foreign-detail", "W04", "GET", "/api/watchlists/" + FOREIGN, 404,
               headers=headers, assert_after=_unchanged),
        Action("missing-detail", "W04", "GET", "/api/watchlists/70000000-0000-4000-8000-000000000099", 404,
               headers=headers, assert_after=_unchanged),
        # A valid bearer reaches path validation; FastAPI's UUID diagnostics must match exactly.
        *(Action(name, "W04", "GET", "/api/watchlists/" + raw, 422, headers=headers, assert_after=_unchanged)
          for name, raw in MALFORMED_UUIDS),
        Action("bad-bearer-before-uuid", "W04", "GET", "/api/watchlists/not-a-uuid", 401,
               headers={"authorization": "Bearer invalid", "x-request-id": REQUEST_ID},
               assert_after=_unchanged),
    ]


def _scenario(name, gate_enabled):
    def servers(context):
        rust_environment = {"WATCHLIST_TEST_CUTOFF": "2026-10-19T00:00:00Z",
                            "KYC_EXECUTION_GATE_ENABLED": "true" if gate_enabled else "false"}
        return [Server((context.python_bin, "-m", "scripts.rust_contract.phase2_oracle", "--port", "{port}")),
                Server((str(context.root / "rs/target/debug/examples/watchlist_server"),), rust_environment)]
    return Scenario(
        name, "watchlists", _setup, servers, lambda context: _actions(context, gate_enabled),
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
               "--features", "contract-tests", "--example", "watchlist_server"),
        route_qualification=True)


def watchlist_reads_scenario():
    """KYC execution gate on (the production default)."""
    return _scenario("watchlist-http-reads", gate_enabled=True)


def watchlist_reads_gate_off_scenario():
    """Same state with the gate off: the gate-dependent order is public."""
    return _scenario("watchlist-http-reads-gate-off", gate_enabled=False)
