"""Task 8 real-route HTTP, full-state, rollback and concurrent write schedules."""
import copy
import json
from datetime import datetime, timedelta, timezone

import jwt

from scripts.rust_contract.phase2_scenarios import (
    Action,
    Race,
    Scenario,
    ScenarioError,
    Server,
)

A = "20000000-0000-4000-8000-00000000000a"
B = "20000000-0000-4000-8000-00000000000b"
C = "20000000-0000-4000-8000-00000000000c"
D = "20000000-0000-4000-8000-00000000000d"
SECRET = "test-secret-key-for-testing-minimum-32-chars"
# History rows sit strictly before the fixture's earliest instant, so the
# fixture rows stay on the first page and the extra rows fill later pages.
HISTORY_START = datetime(2025, 12, 31, 12, 0, tzinfo=timezone.utc)
HISTORY_ID = "31000000-0000-4000-8000-{:012d}"


def token(actor):
    now = datetime.now(timezone.utc)
    return jwt.encode({"sub": actor, "type": "access", "iat": int(now.timestamp()),
                       "exp": int((now + timedelta(hours=2)).timestamp()), "jti": "notification-contract",
                       "iss": "verdaxis-test-api", "aud": "verdaxis-test-web"}, SECRET, algorithm="HS256")


def rows(state):
    return state["notifications"]["rows"]


def assert_changes(before, after, mail, *, actor=None, notification=None):
    expected = copy.deepcopy(before)
    for row in rows(expected).values():
        if row["recipient_id"] == actor and (notification is None or row["id"] == notification):
            row["is_read"] = True
    if expected != after or mail:
        raise ScenarioError("notification state unit, unrelated rows or no-mail invariant failed")


def scenario():
    def setup(connection):
        from pathlib import Path
        fixture = json.loads((Path(__file__).resolve().parents[2] / "tests/rust_contract/phase2/notification-scenario.json").read_text())
        connection.execute("INSERT INTO organizations(id,name,domain,type) VALUES ('10000000-0000-4000-8000-000000000002','Foreign','foreign.invalid','FUEL_BUYER')")
        for actor in fixture["actors"]:
            connection.execute("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES (%s::uuid,%s,'not-a-login','BUYER','APPROVED',%s::uuid,true,false)",
                               (actor["id"], actor["name"] + "@notification.invalid", actor["organization_id"]))
        connection.execute("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES (%s::uuid,'empty@notification.invalid','not-a-login','BUYER','APPROVED','10000000-0000-4000-8000-000000000001',true,false)", (D,))
        actors = {item["name"]: item["id"] for item in fixture["actors"]}
        for item in fixture["fixtures"]:
            connection.execute("INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) VALUES (%s::uuid,%s::uuid,'SYSTEM','Title','Message',%s::json,%s,%s::timestamptz)",
                               (item["id"], actors[item["actor"]], json.dumps(item["data"]) if item["data"] is not None else None, item["is_read"], datetime.fromisoformat(item["created_at"])))
        # A row trigger fails only the requested recipient's update. Statement
        # rollback also undoes updates to rows visited before the failing row.
        connection.execute("""CREATE FUNCTION notification_contract_fault() RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN IF NEW.recipient_id='20000000-0000-4000-8000-00000000000c'::uuid
          AND NEW.id='30000000-0000-4000-8000-0000000000c1'::uuid THEN
          RAISE EXCEPTION 'disposable notification write fault'; END IF; RETURN NEW; END $$""")
        connection.execute("CREATE TRIGGER notification_contract_fault BEFORE UPDATE ON notifications FOR EACH ROW EXECUTE FUNCTION notification_contract_fault()")

    def volume(connection, profile):
        """The paged personal history for actor A; D stays empty."""
        for index in range(profile.notifications):
            connection.execute(
                "INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) "
                "VALUES (%s::uuid,%s::uuid,'SYSTEM',%s,'History',NULL,%s,%s)",
                (HISTORY_ID.format(index), A, "History " + str(index),
                 index % 3 == 0, HISTORY_START - timedelta(seconds=index)))

    def servers(context):
        return [Server((context.python_bin, "-m", "scripts.rust_contract.notifications_oracle", "--port", "{port}")),
                Server((str(context.root / "rs/target/debug/examples/notifications_contract"),))]

    def action(name, route, method, path, actor=A, status=200, notification=None, fault=None, check=None):
        headers = {"authorization": "Bearer " + token(actor), "x-request-id": "notification-" + name}
        if fault:
            headers["x-notification-fault"] = fault
        def assertion(before, after, response, mail):
            assert_changes(before, after, mail, actor=actor if method == "PATCH" and status in (200, 503) else None,
                           notification=notification)
            if route == "N01" and status == 200:
                result = json.loads(response["body"])
                for item in result:
                    if "recipient_id" in item or any(key.startswith("_email_") for key in (item["data"] or {})):
                        raise ScenarioError("notification projection exposed private state")
        return Action(name, route, method, path, status, headers=headers, actor_id=actor,
                      outcome="ambiguous" if fault else "rolled-back" if status >= 400 else "committed",
                      assert_after=check or assertion)

    def actions(context):
        profile = getattr(context, "size", None)
        visible = profile.notifications + 7 if profile is not None else 0

        def read_only(expected=None):
            def assertion(before, after, response, mail):
                if before != after or mail:
                    raise ScenarioError("notification read changed state or sent mail")
                if expected is not None:
                    result = json.loads(response["body"])
                    if len(result) != expected:
                        raise ScenarioError(f"expected {expected} notifications, saw {len(result)}")
            return assertion

        def full_page(before, after, response, mail):
            read_only()(before, after, response, mail)
            result = json.loads(response["body"])
            if len(result) != min(100, visible):
                raise ScenarioError("first page length differs from the page bound")
            stamps = [item["created_at"] for item in result]
            if stamps != sorted(stamps, reverse=True):
                raise ScenarioError("notification page is not newest first")

        def empty_count(before, after, response, mail):
            read_only()(before, after, response, mail)
            if json.loads(response["body"]) != {"count": 0}:
                raise ScenarioError("empty count is not zero")

        result = [action("list-a", "N01", "GET", "/api/notifications"),
                  action("list-empty", "N01", "GET", "/api/notifications", D, status=200, check=read_only(0)),
                  action("count-empty", "N02", "GET", "/api/notifications/unread-count", D, check=empty_count),
                  action("page-full", "N01", "GET", "/api/notifications?limit=100", status=200, check=full_page),
                  action("page-second", "N01", "GET", "/api/notifications?skip=100&limit=100", status=200,
                         # The route caps `limit` at 100, so the second page is
                         # also bounded by the page size, not the remainder.
                         check=read_only(min(100, max(0, visible - 100)))),
                  action("page-beyond-end", "N01", "GET", "/api/notifications?skip=100000&limit=100", status=200,
                         check=read_only(0)),
                  action("list-b", "N01", "GET", "/api/notifications", B),
                  action("list-c", "N01", "GET", "/api/notifications", C),
                  action("page", "N01", "GET", "/api/notifications?skip=2&limit=2"),
                  action("invalid-query", "N01", "GET", "/api/notifications?skip=-1&limit=0", status=422),
                  action("count-before", "N02", "GET", "/api/notifications/unread-count"),
                  action("invalid-id", "N03", "PATCH", "/api/notifications/not-a-uuid/read", status=422)]
        for label, suffix in [("same-org", "b1"), ("cross-org", "c1"), ("missing", "ff")]:
            result.append(action(label, "N03", "PATCH", "/api/notifications/30000000-0000-4000-8000-0000000000" + suffix + "/read", status=404))
        result.extend([action("rollback-single", "N03", "PATCH", "/api/notifications/30000000-0000-4000-8000-0000000000c1/read", C, 500),
                       action("rollback-all", "N04", "PATCH", "/api/notifications/read-all", C, 500)])
        for repeat in range(2):
            result.append(action(f"hidden-{repeat}", "N03", "PATCH", "/api/notifications/30000000-0000-4000-8000-000000000003/read", notification="30000000-0000-4000-8000-000000000003"))
        result.extend([action("response-loss", "N03", "PATCH", "/api/notifications/30000000-0000-4000-8000-000000000001/read", status=503,
                              notification="30000000-0000-4000-8000-000000000001", fault="after-commit"),
                       action("count-after", "N02", "GET", "/api/notifications/unread-count")])
        # Leave ordinary unread rows for the concurrent read/read-all schedule.
        return result

    def races(_context):
        def invariant(before, after, results):
            expected = copy.deepcopy(before)
            for item in rows(expected).values():
                if item["recipient_id"] in (A, B):
                    item["is_read"] = True
            if expected != after:
                raise ScenarioError("concurrent read-state writes changed the wrong unit")
        actors = (action("same-user-single", "N03", "PATCH", "/api/notifications/30000000-0000-4000-8000-000000000002/read", notification="30000000-0000-4000-8000-000000000002"),
                  action("same-user-all", "N04", "PATCH", "/api/notifications/read-all"),
                  action("same-org-all", "N04", "PATCH", "/api/notifications/read-all", B),
                  action("cross-org-rollback", "N04", "PATCH", "/api/notifications/read-all", C, 500))
        return [Race(actors, invariant, repeats=3, seed=20261019), Race(tuple(reversed(actors)), invariant, repeats=3, seed=20261022)]

    return Scenario("notification_scope_and_email_state", "notifications", setup, servers, actions,
                    build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--example", "notifications_contract"),
                    races=races, volume=volume)
