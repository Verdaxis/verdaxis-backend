"""Task 13 schedules: SLICE and PIN target writes (W05, W06).

Both stacks receive the same clock and row identifiers. Each action asserts the
exact committed change. PD1 differs from live Python in HTTP and in committed
rows, so its actions name the rule and pin both stacks' states exactly.
"""
from decimal import Decimal

from scripts.rust_contract.phase2_scenarios import (
    Action,
    Race,
    Scenario,
    ScenarioError,
    Server,
    invalid_bearer_pair,
)
from scripts.rust_contract.phase2_watchlist_asserts import (
    Schedule,
    body_of,
    created,
    no_write,
    rows,
    same_instant,
    uid,
    unchanged_except,
)
from scripts.rust_contract.phase2_watchlist_fixtures import (
    BUSAN,
    DALIAN,
    DEMO_ORGANIZATION,
    FAR_FUTURE,
    GATED_ORDER,
    GATED_USER,
    OWNER,
    PUBLIC_ORDER,
    SHANGHAI,
    UNKNOWN_ORDER,
    at,
    insert_container,
    insert_event,
    insert_order,
    insert_pin,
    insert_slice,
    order_event,
    seed_users_and_orders,
)

REQUEST_ID = "watchlist-target-case"
ACTOR = OWNER
OTHER = GATED_USER
POINT_NAMES = {DALIAN: "Dalian", BUSAN: "Busan", SHANGHAI: "Shanghai"}

# Public orders in distinct slice keys (all B30 at Dalian), plus two that never
# are: a malformed stored window, and the fixture's unknown-provenance order.
PARTIAL_ORDER = uid(74, 4)  # SPOT, partly filled
MONTH_ORDER = uid(74, 5)  # 2026-12
FAULT_ORDER = uid(74, 6)  # 2027-CAL, price 777.77: a fixture trigger fails its PIN insert
ORPHAN_ORDER = uid(74, 7)  # 2027-Q1, already pinned in ORPHAN without a companion slice
BAD_WINDOW_ORDER = uid(74, 8)  # stored window outside the grammar

MAIN = uid(70, 1)
ORPHAN = uid(70, 2)
DELETES = uid(70, 3)
KEEP = uid(70, 4)
FOREIGN = uid(70, 5)
RACE = uid(70, 6)
MISSING = uid(70, 99)

# Targets seeded in DELETES (and one pair in KEEP).
SLICE_SPOT, PIN_PUBLIC, PIN_PARTIAL = uid(72, 11), uid(72, 12), uid(72, 13)
SLICE_MONTH, PIN_MONTH = uid(72, 14), uid(72, 15)
KEEP_SLICE, KEEP_PIN = uid(72, 21), uid(72, 22)
ORPHAN_PIN = uid(72, 31)

ORDERS = {
    PUBLIC_ORDER: ("SPOT", "600", "OPEN", "100", "100"),
    PARTIAL_ORDER: ("SPOT", "601.25", "PARTIALLY_FILLED", "100", "40.5"),
    MONTH_ORDER: ("2026-12", "600", "OPEN", "100", "100"),
    UNKNOWN_ORDER: ("SPOT", "600", "OPEN", "100", "100"),
    GATED_ORDER: ("SPOT", "600", "OPEN", "100", "100"),
}
ORDER_POINTS = {PUBLIC_ORDER: DALIAN, PARTIAL_ORDER: DALIAN, MONTH_ORDER: DALIAN,
                UNKNOWN_ORDER: BUSAN, GATED_ORDER: SHANGHAI}


def _setup(connection):
    seed_users_and_orders(connection)
    extra = ((PARTIAL_ORDER, "SPOT", "601.25", "PARTIALLY_FILLED", "100", "40.5"),
             (MONTH_ORDER, "2026-12", "600", "OPEN", "100", "100"),
             (FAULT_ORDER, "2027-CAL", "777.77", "OPEN", "100", "100"),
             (ORPHAN_ORDER, "2027-Q1", "600", "OPEN", "100", "100"),
             (BAD_WINDOW_ORDER, "bad-window", "600", "OPEN", "100", "100"))
    for order, window, price, status, quantity, remaining in extra:
        insert_order(connection, order, DEMO_ORGANIZATION, None, "DEMO", DALIAN, FAR_FUTURE, window=window,
                     price=Decimal(price), status=status, quantity=Decimal(quantity), remaining=Decimal(remaining))
    insert_container(connection, MAIN, ACTOR, "Main", at(minutes=1))
    insert_container(connection, ORPHAN, ACTOR, "Pin without companion", at(minutes=2))
    insert_pin(connection, ORPHAN_PIN, ORPHAN, ORPHAN_ORDER, DALIAN, at(minutes=2), "2027-Q1")
    insert_container(connection, RACE, ACTOR, "Race", at(minutes=5))
    insert_container(connection, FOREIGN, OTHER, "Foreign", at(minutes=6))
    for container, minute in ((DELETES, 3), (KEEP, 4)):
        insert_container(connection, container, ACTOR, "Deletes" if container == DELETES else "Keep", at(minutes=minute))
    for target, container, product_window, order in (
            (SLICE_SPOT, DELETES, "SPOT", None), (PIN_PUBLIC, DELETES, "SPOT", PUBLIC_ORDER),
            (PIN_PARTIAL, DELETES, "SPOT", PARTIAL_ORDER), (SLICE_MONTH, DELETES, "2026-12", None),
            (PIN_MONTH, DELETES, "2026-12", MONTH_ORDER), (KEEP_SLICE, KEEP, "SPOT", None),
            (KEEP_PIN, KEEP, "SPOT", PUBLIC_ORDER)):
        if order is None:
            insert_slice(connection, target, container, "B30", DALIAN, product_window, at(minutes=7))
        else:
            insert_pin(connection, target, container, order, DALIAN, at(minutes=8), product_window)
        insert_event(connection, uid(73, int(target[-3:])), container, target, "PIN_PRICE_CHANGED",
                     order_event(order or PUBLIC_ORDER), at(days=1))
    # Fixture-owned SQL fault: the real handlers both fail on this PIN insert, after
    # the companion slice was written, before commit.
    connection.execute("""CREATE FUNCTION watchlist_target_failure() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF NEW.target_type = 'PIN' AND NEW.snapshot_price_per_mt_usd = 777.77 THEN
        RAISE EXCEPTION 'disposable pin write failure';
      END IF;
      RETURN NEW;
    END $$""")
    connection.execute("CREATE TRIGGER watchlist_target_failure BEFORE INSERT ON watchlist_targets "
                       "FOR EACH ROW EXECUTE FUNCTION watchlist_target_failure()")


# ---------------------------------------------------------------- expected rows

NO_SNAPSHOT = {"snapshot_price_per_mt_usd": None, "snapshot_quantity_mt": None,
               "snapshot_remaining_quantity_mt": None, "snapshot_status": None, "snapshot_side": None}


def _slice_row(identifier, container, product, point, window, *, snapshot_product=None):
    return {"id": identifier, "watchlist_id": container, "target_type": "SLICE",
            "market_product_code": product, "delivery_point_id": point, "availability_window_code": window,
            "order_id": None, **NO_SNAPSHOT, "snapshot_market_product": snapshot_product or product,
            "snapshot_delivery_point_name": POINT_NAMES[point], "snapshot_availability_window": window,
            "snapshot_counterparty_label": None}


def _pin_row(identifier, container, order, window=None):
    window = window or ORDERS[order][0]
    _, price, status, quantity, remaining = ORDERS[order]
    point = ORDER_POINTS[order]
    return {"id": identifier, "watchlist_id": container, "target_type": "PIN", "market_product_code": "B30",
            "delivery_point_id": point, "availability_window_code": window, "order_id": order,
            "snapshot_price_per_mt_usd": float(price), "snapshot_quantity_mt": float(quantity),
            "snapshot_remaining_quantity_mt": float(remaining), "snapshot_status": status,
            "snapshot_side": "BID", "snapshot_market_product": "B30",
            "snapshot_delivery_point_name": POINT_NAMES[point], "snapshot_availability_window": window,
            "snapshot_counterparty_label": None}


def _response_fields(row):
    fields = {key: value for key, value in row.items() if key != "watchlist_id"}
    fields["delivery_point_name"] = row["snapshot_delivery_point_name"]
    fields.update(active_order_count=0, unread_event_count=0, latest_event_at=None)
    return fields


def _commits(clock, *expected):
    """Exactly these target rows are new; nothing else changed and no event was created."""
    def check(before, after, response, mail):
        unchanged_except(before, after, "watchlist_targets")
        new = sorted(created(before, after, "watchlist_targets"), key=lambda row: row["target_type"])
        wanted = sorted(expected, key=lambda row: row["target_type"])
        if len(new) != len(wanted):
            raise ScenarioError(f"expected {len(wanted)} new targets, found {len(new)}")
        for row, want in zip(new, wanted, strict=True):
            if {key: row[key] for key in want} != want or not same_instant(row["created_at"], clock):
                raise ScenarioError("committed target differs from the controlled values")
        body = body_of(response)
        reported = next((row for row in wanted if row["target_type"] == "PIN"), wanted[0])
        want = _response_fields(reported)
        if {key: body[key] for key in want} != want or not same_instant(body["created_at"], clock):
            raise ScenarioError("response differs from the committed target")
        if mail:
            raise ScenarioError("target write sent mail")
    return check


def _detail(text):
    def check(before, after, response, mail):
        no_write(before, after, response, mail)
        if body_of(response) != {"detail": text}:
            raise ScenarioError("refusal text differs")
    return check


# ---------------------------------------------------------------- PD1 pairs

def _hidden_order_pair(order, container, companion, pin, *, python_status=201, python_text=None):
    """PD1: live Python pins an order outside the public scope; Rust says it was not found."""
    def http(responses):
        python, rust = responses
        if python["status"] != python_status or rust["status"] != 404:
            raise ScenarioError(f"statuses differ from the approved pair: {python['status']}, {rust['status']}")
        if body_of(rust) != {"detail": "Order not found"}:
            raise ScenarioError("Rust refusal text differs")
        if python_text is not None:
            if python["body"] != python_text:
                raise ScenarioError("Python unhandled-error body differs")
        else:
            body = body_of(python)
            if (body["id"], body["order_id"], body["target_type"]) != (pin, order, "PIN"):
                raise ScenarioError("Python stored a different PIN")

    def state(before, after):
        # Earlier pairs in this scenario may already have left the twins apart
        # by design. Each side is pinned against its own before.
        if after[1] != before[1]:
            raise ScenarioError("Rust stored a row for an order outside the public scope")
        if python_text is not None:
            if after[0] != before[0]:
                raise ScenarioError("Python changed state on its unhandled error")
            return
        unchanged_except(before[0], after[0], "watchlist_targets")
        stored = {row["target_type"]: row for row in created(before[0], after[0], "watchlist_targets")}
        if set(stored) != {"SLICE", "PIN"} or stored["PIN"]["id"] != pin or stored["SLICE"]["id"] != companion \
                or stored["PIN"]["order_id"] != order or stored["PIN"]["watchlist_id"] != container:
            raise ScenarioError("Python stored a different companion and PIN")
    return http, state


class _Builder:
    def __init__(self, secret):
        self.schedule = Schedule(secret, REQUEST_ID)

    def write(self, name, route, method, path, status, user=ACTOR, *, ids=(), body=None, raw=None,
              check=None, outcome="committed", token=None, expect=None, **pair):
        headers, clock = self.schedule.headers(user, ids, token=token)
        if expect is not None:
            check = expect(clock)
        return Action(name, route, method, path, status, body=body, raw_body=raw, headers=headers,
                      actor_id=user, outcome=outcome, assert_after=check, **pair)

    def post(self, name, status, body=None, *, container=MAIN, **options):
        path = f"/api/watchlists/{container}/targets"
        if status not in (201, 409, 500):
            options.setdefault("check", no_write)
        return self.write(name, "W05", "POST", path, status, body=body, **options)


def _slice_body(product, point, window):
    return {"target_type": "SLICE", "market_product_code": product, "delivery_point_id": point,
            "availability_window_code": window}


def _pin_body(order):
    return {"target_type": "PIN", "order_id": order}


def _deleted(container, removed, kept):
    def check(before, after, response, mail):
        if response["body"] != b"" or mail:
            raise ScenarioError("delete must return an empty body and send no mail")
        unchanged_except(before, after, "watchlist_targets", "watchlist_events")
        present = {row["id"] for row in rows(after, "watchlist_targets")}
        if present & set(removed) or not set(kept) <= present:
            raise ScenarioError("wrong targets removed")
        for row in rows(after, "watchlist_events"):
            if row["watchlist_target_id"] in removed:
                raise ScenarioError("events of a removed target survived")
        gone = {row["id"] for row in rows(before, "watchlist_events") if row["watchlist_target_id"] in removed}
        survivors = [row for row in rows(before, "watchlist_events") if row["id"] not in gone]
        if survivors != rows(after, "watchlist_events"):
            raise ScenarioError("deletion changed events of other targets")
        if len(rows(before, "watchlist_targets")) - len(rows(after, "watchlist_targets")) != len(removed):
            raise ScenarioError("deletion removed other targets")
    return check


def _actions_for(context, gate_enabled):
    build = _Builder(context.environment["JWT_SECRET"])
    ids = uid
    mark = _commits

    def post(name, status, body=None, **options):
        return build.post(name, status, body, **options)

    # ---- SLICE
    yield post("slice-created", 201, _slice_body("B100", DALIAN, "SPOT"), ids=[ids(82, 1)],
               expect=lambda clock: mark(clock, _slice_row(ids(82, 1), MAIN, "B100", DALIAN, "SPOT")))
    yield post("slice-duplicate", 409, _slice_body("B100", DALIAN, "SPOT"), ids=[ids(82, 2)],
               outcome="rolled-back", check=_detail("Duplicate watchlist target"))
    yield post("slice-alias-window-normalized", 201, _slice_body("B100", DALIAN, "Q1 2026"), ids=[ids(82, 3)],
               expect=lambda clock: mark(clock, _slice_row(ids(82, 3), MAIN, "B100", DALIAN, "2026-Q1")))
    yield post("slice-padded-lowercase-spot", 201, _slice_body("B100", BUSAN, "  spot "), ids=[ids(82, 4)],
               expect=lambda clock: mark(clock, _slice_row(ids(82, 4), MAIN, "B100", BUSAN, "SPOT")))
    yield post("slice-malformed-window", 422, _slice_body("B100", DALIAN, "bad-window"),
               check=_detail("availability_window must be SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL"))
    yield post("slice-blank-window", 422, _slice_body("B100", DALIAN, "   "),
               check=_detail("availability_window cannot be blank"))
    yield post("slice-unknown-product", 422, _slice_body("NOT_A_PRODUCT", DALIAN, "SPOT"),
               check=_detail("Invalid market_product_code"))
    yield post("slice-missing-point", 404, _slice_body("B100", MISSING, "SPOT"),
               check=_detail("Delivery point not found"))
    yield post("slice-point-checked-before-window", 404, _slice_body("B100", MISSING, "bad-window"),
               check=_detail("Delivery point not found"))
    yield post("slice-window-checked-before-product", 422, _slice_body("NOT_A_PRODUCT", DALIAN, "bad-window"),
               check=_detail("availability_window must be SPOT, YYYY-MM, YYYY-QN, or YYYY-CAL"))
    yield post("slice-foreign-container", 404, _slice_body("B100", DALIAN, "SPOT"), container=FOREIGN,
               check=_detail("Watchlist not found"))
    yield post("slice-missing-container", 404, _slice_body("B100", DALIAN, "SPOT"), container=MISSING,
               check=_detail("Watchlist not found"))

    # ---- PIN
    companion, pin = ids(83, 1), ids(83, 2)
    yield post("pin-public-order-adds-companion-slice", 201, _pin_body(PUBLIC_ORDER), ids=[companion, pin],
               expect=lambda clock: mark(clock, _slice_row(companion, MAIN, "B30", DALIAN, "SPOT"),
                                         _pin_row(pin, MAIN, PUBLIC_ORDER)))
    yield post("pin-partly-filled-order-reuses-companion", 201, _pin_body(PARTIAL_ORDER), ids=[ids(83, 3)],
               expect=lambda clock: mark(clock, _pin_row(ids(83, 3), MAIN, PARTIAL_ORDER)))
    yield post("pin-duplicate", 409, _pin_body(PUBLIC_ORDER), ids=[ids(83, 4)], outcome="rolled-back",
               check=_detail("Duplicate watchlist target"))
    yield post("pin-forward-month-adds-companion-slice", 201, _pin_body(MONTH_ORDER), ids=[ids(83, 5), ids(83, 6)],
               expect=lambda clock: mark(clock, _slice_row(ids(83, 5), MAIN, "B30", DALIAN, "2026-12"),
                                         _pin_row(ids(83, 6), MAIN, MONTH_ORDER)))
    yield post("pin-duplicate-leaves-no-companion-slice", 409, _pin_body(ORPHAN_ORDER), container=ORPHAN,
               ids=[ids(83, 7), ids(83, 8)], outcome="rolled-back", check=_detail("Duplicate watchlist target"))
    yield post("pin-failure-between-companion-and-pin", 500, _pin_body(FAULT_ORDER), ids=[ids(83, 9), ids(83, 10)],
               outcome="rolled-back", check=_detail("Database operation failed."))
    yield post("pin-missing-order", 404, _pin_body(MISSING), check=_detail("Order not found"))
    yield post("pin-foreign-container", 404, _pin_body(PUBLIC_ORDER), container=FOREIGN,
               check=_detail("Watchlist not found"))

    if not gate_enabled:
        gated = (ids(85, 1), ids(85, 2))
        yield post("pin-order-gate-off-visible", 201, _pin_body(GATED_ORDER), ids=list(gated),
                   expect=lambda clock: mark(clock, _slice_row(gated[0], MAIN, "B30", SHANGHAI, "SPOT"),
                                             _pin_row(gated[1], MAIN, GATED_ORDER)))

    # ---- request validation: nothing is written
    for name, body in (
            ("slice-missing-fields", {"target_type": "SLICE"}),
            ("slice-wrong-types", {"target_type": "SLICE", "market_product_code": 5, "delivery_point_id": 7,
                                   "availability_window_code": None}),
            ("slice-malformed-point-id", _slice_body("B100", "not-a-uuid", "SPOT")),
            ("pin-missing-order-id", {"target_type": "PIN"}),
            ("pin-malformed-order-id", {"target_type": "PIN", "order_id": "not-a-uuid"}),
            ("pin-order-id-wrong-type", {"target_type": "PIN", "order_id": 7}),
            ("target-type-missing", {"order_id": PUBLIC_ORDER}),
            ("target-type-unknown", {"target_type": "slice", "order_id": PUBLIC_ORDER}),
            ("target-type-number", {"target_type": 5}),
            ("target-type-null", {"target_type": None}),
            ("target-type-boolean", {"target_type": True}),
            ("body-array", ["SLICE"]), ("body-string", "SLICE")):
        yield post(name, 422, body)
    yield post("body-null", 422, raw=b"null")
    yield post("body-empty", 422, raw=b"")
    yield build.write("malformed-path-and-body", "W05", "POST", "/api/watchlists/not-a-uuid/targets", 422,
                      body={"order_id": PUBLIC_ORDER}, check=no_write)
    yield build.write("malformed-path-only", "W05", "POST", "/api/watchlists/not-a-uuid/targets", 422,
                      body=_pin_body(PUBLIC_ORDER), check=no_write)
    yield post("bad-bearer-before-validation", 401, {}, token="Bearer invalid",
               approved_difference="PD89-CF4-11", assert_http_difference=invalid_bearer_pair())

    # ---- deletion
    def delete(name, status, target, container=DELETES, **options):
        options.setdefault("check", no_write)
        return build.write(name, "W06", "DELETE", f"/api/watchlists/{container}/targets/{target}", status,
                           **options)

    yield delete("delete-slice-removes-matching-pins", 204, SLICE_SPOT,
                 check=_deleted(DELETES, {SLICE_SPOT, PIN_PUBLIC, PIN_PARTIAL}, {SLICE_MONTH, PIN_MONTH, KEEP_SLICE, KEEP_PIN}))
    yield delete("delete-pin-alone", 204, PIN_MONTH,
                 check=_deleted(DELETES, {PIN_MONTH}, {SLICE_MONTH, KEEP_SLICE, KEEP_PIN}))
    yield delete("delete-repeat", 404, SLICE_SPOT, check=_detail("Target not found"))
    yield delete("delete-missing-target", 404, MISSING, check=_detail("Target not found"))
    yield delete("delete-target-of-another-container", 404, KEEP_SLICE, check=_detail("Target not found"))
    yield delete("delete-foreign-container", 404, KEEP_SLICE, container=FOREIGN, check=_detail("Watchlist not found"))
    yield delete("delete-malformed-identifiers", 422, "also-not-a-uuid", container="not-a-uuid")
    yield delete("delete-bad-bearer", 401, SLICE_MONTH, token="Bearer invalid",
                 approved_difference="PD89-CF4-11", assert_http_difference=invalid_bearer_pair())


def _races(context):
    schedule = Schedule(context.environment["JWT_SECRET"], REQUEST_ID, day=3)

    def contested(label, body, ids, rows_expected):
        headers, clock = schedule.headers(ACTOR, ids)
        path = f"/api/watchlists/{RACE}/targets"
        actors = tuple(Action(f"{label}-{n}", "W05", "POST", path, 201, body=body, headers=headers,
                              actor_id=ACTOR, accepted_statuses=(201, 409)) for n in ("first", "second"))

        def invariant(before, after, results):
            unchanged_except(before, after, "watchlist_targets")
            statuses = sorted(response["status"] for response, _error in results.values())
            if statuses != [201, 409]:
                raise ScenarioError(f"contested {label} must give one success and one conflict: {statuses}")
            new = created(before, after, "watchlist_targets")
            if sorted(row["id"] for row in new) != sorted(row["id"] for row in rows_expected):
                raise ScenarioError(f"contested {label} must store exactly one set of rows")
            for row, want in zip(sorted(new, key=lambda r: r["id"]), sorted(rows_expected, key=lambda r: r["id"]), strict=True):
                if {key: row[key] for key in want} != want or not same_instant(row["created_at"], clock):
                    raise ScenarioError(f"contested {label} stored different values")
        return Race(actors=actors, invariant=invariant, repeats=1, seed=20261020, contested=True)

    slice_row = _slice_row(uid(86, 1), RACE, "B100", SHANGHAI, "SPOT")
    pin_rows = [_slice_row(uid(86, 2), RACE, "B30", DALIAN, "SPOT"), _pin_row(uid(86, 3), RACE, PUBLIC_ORDER)]

    headers_a, clock_a = schedule.headers(ACTOR, [uid(86, 4)])
    headers_b, clock_b = schedule.headers(ACTOR, [uid(86, 5)])
    path = f"/api/watchlists/{RACE}/targets"
    distinct = tuple(Action(name, "W05", "POST", path, 201, body=_slice_body("B100", point, window),
                            headers=headers, actor_id=ACTOR)
                     for name, point, window, headers in (("distinct-a", BUSAN, "2026-Q2", headers_a),
                                                          ("distinct-b", DALIAN, "2026-Q3", headers_b)))

    def distinct_invariant(before, after, results):
        unchanged_except(before, after, "watchlist_targets")
        new = created(before, after, "watchlist_targets")
        if sorted(row["id"] for row in new) != sorted([uid(86, 4), uid(86, 5)]):
            raise ScenarioError("both distinct targets must be stored")
        if any(error is not None or response["status"] != 201 for response, error in results.values()):
            raise ScenarioError("both distinct requests must succeed")
    return [
        contested("duplicate-slice", _slice_body("B100", SHANGHAI, "SPOT"), [uid(86, 1)], [slice_row]),
        contested("duplicate-pin", _pin_body(PUBLIC_ORDER), [uid(86, 2), uid(86, 3)], pin_rows),
        Race(actors=distinct, invariant=distinct_invariant, repeats=1, seed=20261021),
    ]


def _hidden_actions_for(context):
    """PD1 pairs only: each one knowingly leaves the twins apart, so they run in
    their own scenario, after every action that compares committed state."""
    build = _Builder(context.environment["JWT_SECRET"])
    ids = uid
    hidden = (ids(84, 1), ids(84, 2))
    http, state = _hidden_order_pair(UNKNOWN_ORDER, MAIN, *hidden, python_status=201)
    yield build.write("pin-order-outside-public-scope", "W05", "POST", f"/api/watchlists/{MAIN}/targets", 201,
                      body=_pin_body(UNKNOWN_ORDER), ids=list(hidden), approved_difference="PD1",
                      accepted_statuses=(201, 404), assert_http_difference=http, assert_state_difference=state)
    http, state = _hidden_order_pair(BAD_WINDOW_ORDER, MAIN, ids(84, 3), ids(84, 4), python_status=500,
                                     python_text=b"Internal Server Error")
    yield build.write("pin-order-with-malformed-window", "W05", "POST", f"/api/watchlists/{MAIN}/targets", 500,
                      body=_pin_body(BAD_WINDOW_ORDER), ids=[ids(84, 3), ids(84, 4)],
                      approved_difference="PD1+PD6", accepted_statuses=(500, 404),
                      assert_http_difference=http, assert_state_difference=state)
    gated = (ids(85, 1), ids(85, 2))
    http, state = _hidden_order_pair(GATED_ORDER, MAIN, *gated, python_status=201)
    yield build.write("pin-order-gate-on-hidden", "W05", "POST", f"/api/watchlists/{MAIN}/targets", 201,
                      body=_pin_body(GATED_ORDER), ids=list(gated), approved_difference="PD1",
                      accepted_statuses=(201, 404), assert_http_difference=http, assert_state_difference=state)


def _scenario(name, gate_enabled, with_races, actions=None):
    def servers(context):
        return (
            Server((context.python_bin, "-m", "tests.rust_contract.phase2.watchlist_server", "--port", "{port}")),
            Server((str(context.root / "rs/target/debug/examples/watchlist_server"),),
                   {"WATCHLIST_TEST_CUTOFF": "2026-10-19T00:00:00Z",
                    "KYC_EXECUTION_GATE_ENABLED": "true" if gate_enabled else "false"}),
        )
    return Scenario(
        name, "watchlists", _setup, servers,
        actions or (lambda context: _actions_for(context, gate_enabled)),
        races=_races if with_races else None,
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
               "--features", "contract-tests", "--example", "watchlist_server"),
        route_qualification=True)


def watchlist_targets_scenario():
    """KYC execution gate on (the production default), with the concurrency schedules."""
    return _scenario("watchlist-targets", gate_enabled=True, with_races=True)


def watchlist_targets_gate_off_scenario():
    """The gate-dependent order is public: PIN creation agrees with Python."""
    return _scenario("watchlist-targets-gate-off", gate_enabled=False, with_races=False)


def watchlist_targets_hidden_scenario():
    """PD1 pairs alone; their committed rows differ by design."""
    return _scenario("watchlist-targets-hidden-orders", gate_enabled=True, with_races=False,
                     actions=_hidden_actions_for)

