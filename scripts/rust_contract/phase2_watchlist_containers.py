"""Task 12 schedules: the default radar and custom container writes (W02, W03, W11).

Both stacks receive the same action clock and row identifiers, so every generated
value stays in the response and state comparison. Each action also asserts the
expected committed change; equal twins alone would not prove it.
"""
from scripts.rust_contract.phase2_scenarios import (
    Action,
    Race,
    Scenario,
    ScenarioError,
    Server,
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
    B30,
    DALIAN,
    GATED_USER,
    OWNER,
    PUBLIC_ORDER,
    at,
    insert_container,
    insert_entry,
    insert_event,
    insert_pin,
    insert_slice,
    insert_user,
    order_event,
    seed_users_and_orders,
)

REQUEST_ID = "watchlist-container-case"
ACTOR = OWNER
OTHER = GATED_USER
CAPPED = "20000000-0000-4000-8000-000000000003"
FAILURE = "20000000-0000-4000-8000-000000000004"
RACER = "20000000-0000-4000-8000-000000000005"


# Containers seeded before the run.
CHILDREN = uid(70, 1)  # custom container with entries, targets and events
KEEP = uid(70, 2)  # a second container whose children must survive
FOREIGN = uid(70, 3)  # owned by OTHER
UNDELETABLE = uid(70, 4)  # a fixture trigger makes its deletion fail
CAPPED_RADAR = uid(70, 5)
MISSING = uid(70, 99)


def _setup(connection):
    seed_users_and_orders(connection)
    for user, email in ((CAPPED, "capped"), (FAILURE, "failure"), (RACER, "racer")):
        insert_user(connection, user, f"{email}@watchlist-containers.invalid")
    for owner, container, name, start in (
        (ACTOR, CHILDREN, "Container with children", 1),
        (ACTOR, KEEP, "Container to keep", 2),
        (ACTOR, UNDELETABLE, "Undeletable", 3),
    ):
        insert_container(connection, container, owner, name, at(minutes=start))
        slice_id, pin_id = uid(72, start * 10), uid(72, start * 10 + 1)
        insert_slice(connection, slice_id, container, "B30", DALIAN, "SPOT", at(minutes=start))
        insert_pin(connection, pin_id, container, PUBLIC_ORDER, DALIAN, at(minutes=start + 1))
        insert_entry(connection, uid(75, start), container, B30, DALIAN, at(minutes=start))
        for number, target in enumerate((slice_id, pin_id)):
            insert_event(connection, uid(73, start * 10 + number), container, target,
                         "PIN_PRICE_CHANGED", order_event(PUBLIC_ORDER), at(days=1 + number))
    insert_container(connection, FOREIGN, OTHER, "Foreign container", at(minutes=4))
    # Nine containers of mixed kinds: one more fits, two do not.
    insert_container(connection, CAPPED_RADAR, CAPPED, "Market Radar", at(minutes=5), "RADAR_DEFAULT")
    insert_container(connection, uid(70, 6), CAPPED, "Archived", at(minutes=6), "LEGACY_ARCHIVED")
    for number in range(7):
        insert_container(connection, uid(70, 10 + number), CAPPED, f"Custom {number}", at(minutes=7 + number))
    # Fixture-owned SQL faults: both real handlers reach the write and fail before commit.
    connection.execute("""CREATE FUNCTION watchlist_container_failure() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF TG_OP = 'INSERT' AND NEW.name = 'rollback-create' THEN
        RAISE EXCEPTION 'disposable container create failure';
      END IF;
      IF TG_OP = 'DELETE' AND OLD.name = 'Undeletable' THEN
        RAISE EXCEPTION 'disposable container delete failure';
      END IF;
      RETURN COALESCE(NEW, OLD);
    END $$""")
    connection.execute("CREATE TRIGGER watchlist_container_failure BEFORE INSERT OR DELETE ON watchlists "
                       "FOR EACH ROW EXECUTE FUNCTION watchlist_container_failure()")


def _created_container(user, name, identifier, clock, kind="CUSTOM"):
    def check(before, after, response, mail):
        unchanged_except(before, after, "watchlists")
        (row,) = created(before, after, "watchlists")
        expected = {"id": identifier, "user_id": user, "name": name, "kind": kind}
        if {key: row[key] for key in expected} != expected or not same_instant(row["created_at"], clock):
            raise ScenarioError("created container differs from the controlled values")
        if mail:
            raise ScenarioError("container write sent mail")
        return row
    return check


def _create_response(user, name, identifier, clock):
    base = _created_container(user, name, identifier, clock)

    def check(before, after, response, mail):
        base(before, after, response, mail)
        body = body_of(response)
        if {key: body[key] for key in ("id", "name", "entry_count", "entries")} != {
                "id": identifier, "name": name, "entry_count": 0, "entries": []}:
            raise ScenarioError("create response differs from the committed container")
        if not same_instant(body["created_at"], clock):
            raise ScenarioError("create response clock differs")
    return check


def _radar_response(user, identifier, clock):
    base = _created_container(user, "Market Radar", identifier, clock, "RADAR_DEFAULT")

    def check(before, after, response, mail):
        base(before, after, response, mail)
        body = body_of(response)
        if (body["id"], body["kind"], body["slices"], body["total_slice_count"]) != (
                identifier, "RADAR_DEFAULT", [], 0):
            raise ScenarioError("radar summary differs from the committed radar")
    return check


def _radars(state, user):
    return [row for row in rows(state, "watchlists")
            if row["user_id"] == user and row["kind"] == "RADAR_DEFAULT"]


def _existing_radar(user, identifier):
    def check(before, after, response, mail):
        if before != after or mail:
            raise ScenarioError("an existing radar must not change state")
        (radar,) = _radars(after, user)
        if radar["id"] != identifier or body_of(response)["id"] != identifier:
            raise ScenarioError("the radar identifier is not stable")
    return check


def _other_rows(state, table, container):
    return [row for row in rows(state, table) if row["watchlist_id"] != container]


def _deleted_container(container, kept):
    def check(before, after, response, mail):
        if response["body"] != b"" or mail:
            raise ScenarioError("delete must return an empty body and send no mail")
        unchanged_except(before, after, "watchlists", "watchlist_entries", "watchlist_targets", "watchlist_events")
        if [row for row in rows(after, "watchlists") if row["id"] == container]:
            raise ScenarioError("container survived its deletion")
        for table in ("watchlist_entries", "watchlist_targets", "watchlist_events"):
            if [row for row in rows(after, table) if row["watchlist_id"] == container]:
                raise ScenarioError(f"{table} rows survived their container")
            if _other_rows(before, table, container) != _other_rows(after, table, container):
                raise ScenarioError(f"{table} rows of other containers changed")
        if not [row for row in rows(after, "watchlist_targets") if row["watchlist_id"] == kept]:
            raise ScenarioError("another container lost its targets")
        if created(before, after, "watchlist_events"):
            raise ScenarioError("deletion created an event")
    return check


def _actions(context):
    schedule = Schedule(context.environment["JWT_SECRET"], REQUEST_ID)

    def write(name, route, method, path, status, user, *, ids=(), body=None, raw=None,
              check=None, outcome="committed", fail_summary=False, token=None, clock_check=None):
        headers, clock = schedule.headers(user, ids, fail_summary=fail_summary, token=token)
        return Action(name, route, method, path, status, body=body, raw_body=raw, headers=headers,
                      actor_id=user, outcome=outcome,
                      assert_after=clock_check(clock) if clock_check else check)

    def create(name, status, user, body=None, raw=None, *, ids=(), created=None, **options):
        if created is not None:
            options["clock_check"] = lambda clock: _create_response(user, created, ids[0], clock)
        elif status != 201:
            options.setdefault("check", no_write)
        return write(name, "W03", "POST", "/api/watchlists", status, user, ids=ids, body=body, raw=raw, **options)

    def radar(name, user, ids, **options):
        return write(name, "W02", "POST", "/api/watchlists/me", 200, user, ids=ids, **options)

    def delete(name, status, user, container, **options):
        return write(name, "W11", "DELETE", "/api/watchlists/" + container, status, user, **options)

    yield radar("radar-created", ACTOR, [uid(80, 1)],
                clock_check=lambda clock: _radar_response(ACTOR, uid(80, 1), clock))
    yield radar("radar-repeat-keeps-identifier", ACTOR, [uid(80, 2)],
                check=_existing_radar(ACTOR, uid(80, 1)))
    yield radar("radar-existing-in-full-account", CAPPED, [uid(80, 3)],
                check=_existing_radar(CAPPED, CAPPED_RADAR))
    yield create("create-single-character", 201, ACTOR, {"name": "a"}, ids=[uid(81, 1)], created="a")
    yield create("create-hundred-characters", 201, ACTOR, {"name": "é" * 100}, ids=[uid(81, 2)], created="é" * 100)
    yield create("create-whitespace-only", 201, ACTOR, {"name": "   "}, ids=[uid(81, 3)], created="   ")
    yield create("create-ignores-extra-fields", 201, ACTOR, {"name": "extra", "kind": "RADAR_DEFAULT", "user_id": OTHER},
                 ids=[uid(81, 4)], created="extra")
    yield create("create-empty-name", 422, ACTOR, {"name": ""})
    yield create("create-name-too-long", 422, ACTOR, {"name": "x" * 101})
    yield create("create-name-not-a-string", 422, ACTOR, {"name": 5})
    yield create("create-name-null", 422, ACTOR, {"name": None})
    yield create("create-missing-name", 422, ACTOR, {})
    yield create("create-array-body", 422, ACTOR, ["name"])
    yield create("create-string-body", 422, ACTOR, "name")
    yield create("create-null-body", 422, ACTOR, raw=b"null")
    yield create("create-empty-body", 422, ACTOR, raw=b"")
    yield create("create-bad-bearer", 401, ACTOR, {"name": "refused"}, token="Bearer invalid")
    yield create("create-bad-bearer-before-validation", 401, ACTOR, {}, token="Bearer invalid")
    yield create("create-tenth-container", 201, CAPPED, {"name": "Tenth"}, ids=[uid(81, 5)], created="Tenth")
    yield create("create-eleventh-container", 400, CAPPED, {"name": "Eleventh"})
    yield create("create-database-failure", 500, ACTOR, {"name": "rollback-create"}, ids=[uid(81, 6)],
                 outcome="rolled-back")
    yield _summary_failure(write, FAILURE, uid(80, 4))
    yield delete("delete-default-radar", 400, ACTOR, uid(80, 1), check=no_write)
    yield delete("delete-foreign-container", 404, ACTOR, FOREIGN, check=no_write)
    yield delete("delete-missing-container", 404, ACTOR, MISSING, check=no_write)
    yield delete("delete-malformed-identifier", 422, ACTOR, "not-a-uuid", check=no_write)
    yield delete("delete-database-failure", 500, ACTOR, UNDELETABLE, outcome="rolled-back", check=no_write)
    yield delete("delete-container-with-children", 204, ACTOR, CHILDREN, check=_deleted_container(CHILDREN, KEEP))
    yield delete("delete-repeat", 404, ACTOR, CHILDREN, check=no_write)


def _summary_failure(write, user, identifier):
    """A summary read that fails after the radar write committed keeps the radar."""
    def committed(clock):
        base = _created_container(user, "Market Radar", identifier, clock, "RADAR_DEFAULT")

        def check(before, after, response, mail):
            base(before, after, response, mail)
            if body_of(response) != {"detail": "Database operation failed."}:
                raise ScenarioError("summary failure must be the database error response")
        return check
    return write("radar-summary-failure-after-commit", "W02", "POST", "/api/watchlists/me", 500, user,
                 ids=[identifier], fail_summary=True, outcome="committed", clock_check=committed)


def _races(context):
    schedule = Schedule(context.environment["JWT_SECRET"], REQUEST_ID)
    headers, clock = schedule.headers(RACER, [uid(80, 5)])
    actors = tuple(Action(name, "W02", "POST", "/api/watchlists/me", 200, headers=headers, actor_id=RACER)
                   for name in ("radar-first", "radar-second"))

    def invariant(before, after, results):
        unchanged_except(before, after, "watchlists")
        (radar,) = _radars(after, RACER)
        if radar["id"] != uid(80, 5) or not same_instant(radar["created_at"], clock):
            raise ScenarioError("concurrent radar creation must keep one row and one identifier")
        for response, error in results.values():
            if error is not None or body_of(response)["id"] != uid(80, 5):
                raise ScenarioError("every concurrent radar call must return the one radar")
    return [Race(actors=actors, invariant=invariant, repeats=3, seed=20261019)]


def watchlist_containers_scenario():
    def servers(context):
        return (
            Server((context.python_bin, "-m", "tests.rust_contract.phase2.watchlist_server", "--port", "{port}")),
            Server((str(context.root / "rs/target/debug/examples/watchlist_server"),),
                   {"WATCHLIST_TEST_CUTOFF": "2026-10-19T00:00:00Z"}),
        )

    return Scenario(
        "watchlist-containers", "watchlists", _setup, servers, _actions, races=_races,
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
               "--features", "contract-tests", "--example", "watchlist_server"),
        route_qualification=True)
