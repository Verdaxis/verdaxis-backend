"""Watchlist activity routes: W07 events, W08 read state, W09/W10 entries.

These four mounted operations had no twin-stack scenario before Task 24.  The
schedule also proves the empty container, both cursor pages and the last page at
the selected history size, plus owner scoping for events and entries.
"""
import json
from datetime import UTC, datetime, timedelta
from urllib.parse import quote

from scripts.rust_contract.phase2_scenarios import (
    Action,
    Scenario,
    ScenarioError,
    Server,
)
from scripts.rust_contract.phase2_watchlist_asserts import (
    Schedule,
    created,
    no_write,
    rows,
    uid,
    unchanged_except,
)
from scripts.rust_contract.phase2_watchlist_fixtures import (
    B30,
    DALIAN,
    GATED_USER,
    OWNER,
    at,
    bearer,
    insert_container,
    insert_event,
    insert_slice,
    insert_user,
    order_event,
    seed_users_and_orders,
)

REQUEST_ID = "watchlist-activity"
SECRET = "test-secret-key-for-testing-minimum-32-chars"

EMPTY = "20000000-0000-4000-8000-00000000000e"
RADAR = uid(70, 1)
CUSTOM = uid(70, 2)
FOREIGN = uid(70, 3)
EMPTY_RADAR = uid(70, 4)
SLICE = uid(72, 1)
FOREIGN_SLICE = uid(72, 2)
FIXTURE_EVENT = uid(73, 1)
FOREIGN_EVENT = uid(73, 2)
NEW_ENTRY = uid(75, 9)
MISSING = uid(70, 99)

# History identifiers and instants are fixed so a cursor is a committed fact,
# not a value captured from a previous response.
HISTORY_ID = "74000000-0000-4000-8000-{:012d}"
HISTORY_START = datetime(2025, 12, 31, 12, 0, tzinfo=UTC)
LATEST_EVENT = datetime(2026, 1, 10, 0, 0, tzinfo=UTC)
PAGE = 25


def event_key(instant, identifier):
    return instant.astimezone(UTC).isoformat() + "|" + identifier


def setup(connection):
    seed_users_and_orders(connection)
    insert_user(connection, EMPTY, "activity-empty@watchlist.invalid")
    insert_container(connection, RADAR, OWNER, "Market Radar", at(minutes=1), kind="RADAR_DEFAULT")
    insert_slice(connection, SLICE, RADAR, "B30", DALIAN, "SPOT", at(minutes=1))
    insert_container(connection, CUSTOM, OWNER, "Custom", at(minutes=2))
    insert_container(connection, FOREIGN, GATED_USER, "Foreign", at(minutes=3))
    insert_slice(connection, FOREIGN_SLICE, FOREIGN, "B30", DALIAN, "SPOT", at(minutes=3))
    insert_event(connection, FIXTURE_EVENT, RADAR, SLICE, "PIN_PRICE_CHANGED",
                 order_event(None), LATEST_EVENT)
    insert_event(connection, FOREIGN_EVENT, FOREIGN, FOREIGN_SLICE, "PIN_PRICE_CHANGED",
                 order_event(None), LATEST_EVENT)


def volume(connection, profile):
    """The paged event history; the empty user's radar keeps no event."""
    for index in range(profile.watchlist_events):
        insert_event(connection, HISTORY_ID.format(index), RADAR, SLICE,
                     "PIN_PRICE_CHANGED", order_event(None),
                     HISTORY_START - timedelta(seconds=index))


def _history_instant(index):
    return HISTORY_START - timedelta(seconds=index)


def _cursor(position):
    """Cursor that resumes after the item at `position` (fixture first)."""
    if position == 0:
        return event_key(LATEST_EVENT, FIXTURE_EVENT)
    return event_key(_history_instant(position - 1), HISTORY_ID.format(position - 1))


def _cursor_query(position):
    # A raw `+00:00` in a query string decodes to a space; the request must be
    # percent-encoded exactly as a browser's URLSearchParams would encode it.
    return quote(_cursor(position), safe="")


def _item_id(profile, position):
    return FIXTURE_EVENT if position == 0 else HISTORY_ID.format(position - 1)


def _page_check(profile, *, start, count, next_cursor):
    def check(_before, _after, response, _mail):
        body = json.loads(response["body"])
        if len(body["items"]) != count:
            raise ScenarioError(f"event page length {len(body['items'])} != {count}")
        if body["next_cursor"] != next_cursor:
            raise ScenarioError("event page cursor differs from the seeded position")
        expected = [_item_id(profile, start + offset) for offset in range(count)]
        if [item["id"] for item in body["items"]] != expected:
            raise ScenarioError("event page order differs from the seeded position")
    return check


def _empty_page(_before, _after, response, _mail):
    if json.loads(response["body"]) != {"items": [], "next_cursor": None}:
        raise ScenarioError("empty event page is not the declared empty value")


def _one_entry(before, after, response, mail):
    if mail:
        raise ScenarioError("entry write sent mail")
    unchanged_except(before, after, "watchlist_entries")
    added = created(before, after, "watchlist_entries")
    if len(added) != 1 or added[0]["id"] != NEW_ENTRY or added[0]["product_id"] != B30:
        raise ScenarioError("entry add did not insert the expected row")
    if added[0]["delivery_point_id"] is not None:
        raise ScenarioError("entry add wrote an unexpected delivery point")


def _entry_removed(before, after, response, mail):
    if mail:
        raise ScenarioError("entry remove sent mail")
    unchanged_except(before, after, "watchlist_entries")
    gone = {row["id"] for row in rows(before, "watchlist_entries")} - {
        row["id"] for row in rows(after, "watchlist_entries")}
    if gone != {NEW_ENTRY}:
        raise ScenarioError("entry remove did not delete only the added row")


def _one_radar(before, after, response, mail):
    if mail:
        raise ScenarioError("radar create sent mail")
    unchanged_except(before, after, "watchlists")
    added = created(before, after, "watchlists")
    if len(added) != 1 or added[0]["id"] != EMPTY_RADAR or added[0]["kind"] != "RADAR_DEFAULT":
        raise ScenarioError("radar create did not insert the default container")


def _event_read(before, after, response, mail):
    if mail:
        raise ScenarioError("event read sent mail")
    unchanged_except(before, after, "watchlist_events")
    before_rows = {row["id"]: row for row in rows(before, "watchlist_events")}
    after_rows = {row["id"]: row for row in rows(after, "watchlist_events")}
    if set(before_rows) != set(after_rows):
        raise ScenarioError("event read changed the event set")
    touched = [identifier for identifier in before_rows
               if before_rows[identifier] != after_rows[identifier]]
    if touched != [FIXTURE_EVENT] or not after_rows[FIXTURE_EVENT]["is_read"]:
        raise ScenarioError("event read did not mark the expected event")


def actions(context):
    profile = context.size
    schedule = Schedule(SECRET, REQUEST_ID)
    auth = {"authorization": bearer(SECRET, OWNER), "x-request-id": REQUEST_ID}
    empty_auth = {"authorization": bearer(SECRET, EMPTY), "x-request-id": REQUEST_ID}
    total = profile.watchlist_events + 1
    radar_headers, _clock = schedule.headers(EMPTY, [EMPTY_RADAR])
    entry_headers, _clock = schedule.headers(OWNER, [NEW_ENTRY])
    return [
        Action("list-empty", "W01", "GET", "/api/watchlists", 200,
               headers=empty_auth, assert_after=no_write),
        Action("radar-empty-create", "W02", "POST", "/api/watchlists/me", 200,
               headers=radar_headers, actor_id=EMPTY, assert_after=_one_radar),
        Action("events-empty", "W07", "GET", "/api/watchlists/" + EMPTY_RADAR + "/events?limit=25", 200,
               headers=empty_auth, assert_after=_empty_page),
        Action("events-first-page", "W07", "GET",
               f"/api/watchlists/{RADAR}/events?limit={PAGE}", 200, headers=auth,
               assert_after=_page_check(profile, start=0, count=PAGE, next_cursor=_cursor(PAGE - 1))),
        Action("events-second-page", "W07", "GET",
               f"/api/watchlists/{RADAR}/events?limit={PAGE}&cursor=" + _cursor_query(PAGE - 1), 200,
               headers=auth,
               assert_after=_page_check(profile, start=PAGE, count=PAGE, next_cursor=_cursor(2 * PAGE - 1))),
        Action("events-last-page", "W07", "GET",
               f"/api/watchlists/{RADAR}/events?limit={PAGE}&cursor=" + _cursor_query(total - 11), 200,
               headers=auth,
               assert_after=_page_check(profile, start=total - 10, count=10, next_cursor=None)),
        Action("events-invalid-cursor", "W07", "GET",
               f"/api/watchlists/{RADAR}/events?limit=25&cursor=not-a-cursor", 422,
               headers=auth, assert_after=no_write),
        Action("event-mark-read", "W08", "PATCH",
               f"/api/watchlists/{RADAR}/events/{FIXTURE_EVENT}", 200, headers=auth,
               assert_after=_event_read),
        Action("event-foreign", "W08", "PATCH",
               f"/api/watchlists/{FOREIGN}/events/{FOREIGN_EVENT}", 404, headers=auth,
               assert_after=no_write),
        Action("entry-add", "W09", "POST", f"/api/watchlists/{CUSTOM}/entries", 201,
               body={"product_id": B30}, headers=entry_headers, actor_id=OWNER,
               assert_after=_one_entry),
        Action("entry-add-duplicate", "W09", "POST", f"/api/watchlists/{CUSTOM}/entries", 201,
               body={"product_id": B30}, headers=entry_headers, actor_id=OWNER,
               assert_after=no_write),
        Action("entry-remove", "W10", "DELETE", f"/api/watchlists/{CUSTOM}/entries/{NEW_ENTRY}", 204,
               headers=auth, assert_after=_entry_removed),
        Action("entry-remove-missing", "W10", "DELETE",
               f"/api/watchlists/{CUSTOM}/entries/{MISSING}", 404, headers=auth,
               assert_after=no_write),
        Action("entry-add-foreign", "W09", "POST", f"/api/watchlists/{FOREIGN}/entries", 404,
               body={"product_id": B30}, headers=entry_headers, actor_id=OWNER,
               assert_after=no_write),
    ]


def servers(context):
    return (
        Server((context.python_bin, "-m", "tests.rust_contract.phase2.watchlist_server",
                "--port", "{port}")),
        Server((str(context.root / "rs/target/debug/examples/watchlist_server"),),
               {"WATCHLIST_TEST_CUTOFF": "2026-10-19T00:00:00Z"}),
    )


def scenario():
    return Scenario(
        "watchlist-activity", "watchlists", setup, servers, actions,
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked",
               "-p", "verdaxis-read", "--features", "contract-tests",
               "--example", "watchlist_server"),
        volume=volume,
        route_qualification=True,
    )
