"""Real feedback route qualification with full-state assertions after each call."""
import json
import time
import uuid
from datetime import datetime

import jwt

from scripts.rust_contract.phase2_scenarios import Action, Scenario, Server

BUYER = "20000000-0000-4000-8000-000000000001"
ADMIN = "20000000-0000-4000-8000-000000000002"
INSTANT = "2026-10-19T12:00:00+00:00"
SECRET = "test-secret-key-for-testing-minimum-32-chars"


def setup(connection):
    connection.execute("UPDATE users SET first_name='Ada',last_name='Lovelace' WHERE id=%s", (BUYER,))
    connection.execute("INSERT INTO users (id,email,password_hash,role,status,email_verified,must_change_password) VALUES (%s,'admin@phase2.invalid','not-a-login','ADMIN','APPROVED',true,false)", (ADMIN,))
    # Admin-authored feedback has no organization or name; tied rows retain identity.
    for index in range(2):
        connection.execute("INSERT INTO feedback_entries (id,user_id,message,page,created_at) VALUES (%s,%s,%s,NULL,%s)",
                           (str(uuid.UUID(int=900 + index)), ADMIN, f"seed-{index}", datetime.fromisoformat(INSTANT)))
    # This fixture-only constraint fails after admission and INSERT, proving rollback.
    connection.execute("ALTER TABLE feedback_entries ADD CONSTRAINT feedback_contract_failure CHECK (message <> 'force-database-error')")


def volume(connection, profile):
    """A longer admin page, strictly older than the pinned action instant."""
    from datetime import timedelta
    base = datetime.fromisoformat(INSTANT)
    for index in range(profile.feedback_history):
        connection.execute(
            "INSERT INTO feedback_entries (id,user_id,message,page,created_at) VALUES (%s,%s,%s,NULL,%s)",
            (str(uuid.UUID(int=10_000 + index)), BUYER, f"history-{index}", base - timedelta(minutes=index + 1)))


def servers(context):
    env = {"JWT_ISSUER": "verdaxis", "JWT_AUDIENCE": "verdaxis-api"}
    return (
        Server((context.python_bin, "-m", "scripts.rust_contract.feedback_oracle", "--port", "{port}"), env),
        Server((str(context.root / "rs/target/debug/examples/feedback_contract"),), env),
    )


def rows(state):
    return state["feedback_entries"]["rows"]


def unchanged(before, after, response, mail):
    assert before == after
    assert not mail


def inserted(identifier, message, page):
    def check(before, after, response, mail):
        assert not mail
        for table in before:
            if table != "feedback_entries":
                assert before[table] == after[table], table
        added = [value for key, value in rows(after).items() if key not in rows(before)]
        assert len(added) == 1
        row = added[0]
        assert row["id"] == identifier and row["user_id"] == BUYER
        assert row["message"] == message and row["page"] == page
        assert row["created_at"] == INSTANT
        assert len(rows(after)) == len(rows(before)) + 1
    return check


def actions(context):
    now = int(time.time())
    def token(subject):
        # The Rust cell validates the token against the pinned action clock,
        # while the Python cell uses the wall clock. Keep the claims valid for
        # both so the scenario does not depend on the day it runs.
        instant = int(datetime.fromisoformat(INSTANT).timestamp())
        issued_at = min(now, instant) - 60
        expires_at = max(now, instant) + 86400
        return jwt.encode({"sub": subject, "type": "access", "iss": "verdaxis", "aud": "verdaxis-api", "iat": issued_at, "exp": expires_at}, SECRET, algorithm="HS256")
    serial = 0
    def action(name, status, body=None, *, subject=BUYER, path="/api/feedback", method="POST", outcome="committed", check=unchanged, extra=None):
        nonlocal serial
        serial += 1
        identifier = str(uuid.UUID(int=serial + 1000))
        headers = {"authorization": "Bearer " + token(subject), "x-feedback-clock": INSTANT,
                   "x-feedback-id": identifier, "x-request-id": "feedback-" + name,
                   "x-forwarded-for": f"192.0.2.{serial}"}
        headers.update(extra or {})
        if isinstance(check, tuple):
            check = inserted(identifier, *check)
        return Action(name, "F02" if method == "GET" else "F01", method, path, status,
                      body=body, headers=headers, outcome=outcome, assert_after=check)

    # Invalid typed bodies consume no endpoint quota and change no business state.
    fixture = json.loads((context.root / "tests/rust_contract/phase2/feedback.json").read_text())
    for case in fixture["submission_cases"]:
        if case["result"] == "rejected" or isinstance(case["body"], dict):
            accepted = case["result"] == "accepted"
            yield action("schema-" + case["id"], 201 if accepted else 422, case["body"],
                         check=(case["message"], case.get("page")) if accepted else unchanged)
    for index in range(2):
        yield action(f"repeat-{index}", 201, {"message": "  repeat  ", "page": "/app"},
                     extra={"idempotency-key": "ignored"}, check=("repeat", "/app"))
    yield action("rollback", 500, {"message": "force-database-error"}, outcome="rolled-back")
    yield action("retained-commit", 500, {"message": "retained once"}, outcome="ambiguous",
                 extra={"x-feedback-response-loss": "1"}, check=("retained once", None))
    yield action("admin-denied", 403, method="GET", path="/api/admin/feedback")
    def page(limit, offset):
        def listing(before, after, response, mail):
            unchanged(before, after, response, mail)
            body = json.loads(response["body"])
            assert body["total"] == len(rows(after))
            total = len(rows(after))
            assert len(body["items"]) == min(limit, max(0, total - offset))
        return listing

    # (query, status, effective limit). The un-queried listing uses the route's
    # default limit of 50, not the maximum of 200.
    for query, expected, limit in (("", 200, 50), ("?limit=200", 200, 200), ("?offset=999", 200, 50),
                                   ("?limit=0", 422, 0), ("?limit=201", 422, 0), ("?offset=-1", 422, 0),
                                   ("?limit=200&offset=0", 200, 200), ("?limit=200&offset=200", 200, 200)):
        from urllib.parse import parse_qs
        parsed = parse_qs(query.lstrip("?"))
        offset = int(parsed.get("offset", ["0"])[0]) if parsed else 0
        yield action("admin-" + str(serial), expected,
                     subject=ADMIN, method="GET", path="/api/admin/feedback" + query,
                     check=unchanged if expected == 422 else page(limit, offset))


def feedback_scenario():
    return Scenario("feedback-routes", "feedback", setup, servers, actions,
                    build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--features", "contract-tests", "--example", "feedback_contract"),
                    volume=volume)
