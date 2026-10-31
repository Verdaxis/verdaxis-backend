"""Task 9 schedules for the shared route runner; no lifecycle or dispatch owner."""
import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

from scripts.rust_contract.phase2_scenarios import Action, Race, Scenario, Server

ROOT = Path(__file__).resolve().parents[2]
ACTOR = "20000000-0000-4000-8000-000000000001"
OTHER = "20000000-0000-4000-8000-000000000002"


def setup(connection):
    connection.execute("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES (%s,'other@preferences.invalid','not-a-login','BUYER','APPROVED','10000000-0000-4000-8000-000000000001',true,false)", (OTHER,))
    connection.execute("INSERT INTO user_preferences(id,user_id,namespace,value,updated_at) VALUES ('30000000-0000-4000-8000-000000000001',%s,'legacy_unknown','[1,{\"keep\":null}]','2026-10-18T00:00:00Z')", (ACTOR,))
    # Fixture-owned SQL fault: both real handlers reach the write and fail before
    # commit. The runtime role cannot create or bypass this trigger.
    connection.execute("""CREATE FUNCTION preferences_test_failure() RETURNS trigger LANGUAGE plpgsql AS $$
    BEGIN
      IF NEW.value::jsonb->'portIds' = '["rollback"]'::jsonb THEN
        RAISE EXCEPTION 'disposable preference write failure';
      END IF;
      RETURN NEW;
    END $$""")
    connection.execute("CREATE TRIGGER preferences_test_failure BEFORE INSERT OR UPDATE ON user_preferences FOR EACH ROW EXECUTE FUNCTION preferences_test_failure()")


def servers(context):
    return (
        Server((context.python_bin, "-m", "tests.rust_contract.phase2.preferences_server", "--port", "{port}")),
        Server((str(context.root / "rs/target/debug/examples/preferences_server"),)),
    )


def headers(index, user=ACTOR):
    import jwt
    now = datetime.now(UTC)
    token = jwt.encode({"sub": user, "type": "access", "iss": "verdaxis-test-api", "aud": "verdaxis-test-web",
                        "iat": int(now.timestamp()), "exp": int((now + timedelta(hours=2)).timestamp())},
                       "test-secret-key-for-testing-minimum-32-chars", algorithm="HS256")
    return {"authorization": "Bearer " + token, "x-request-id": "preferences-contract",
            "x-test-preference-clock": (datetime(2026, 10, 19, tzinfo=UTC) + timedelta(seconds=index)).isoformat(),
            "x-test-preference-id": f"40000000-0000-4000-8000-{index:012d}",
            "x-forwarded-for": f"192.0.2.{index % 250 + 1}"}


def rows(state):
    return list(state["user_preferences"]["rows"].values())


def assertion(user, namespace=None, value=None, clock=None, unchanged=False):
    def check(before, after, response, mail):
        assert mail == [], "preferences sent mail"
        for table in before:
            if table != "user_preferences" or unchanged:
                assert before[table] == after[table], f"unexpected write: {table}"
        if namespace is not None and not unchanged:
            selected = [row for row in rows(after) if row["user_id"] == user and row["namespace"] == namespace]
            assert len(selected) == 1
            assert selected[0]["value"] == value
            assert datetime.fromisoformat(selected[0]["updated_at"]) == datetime.fromisoformat(clock)
            assert [row for row in rows(before) if row["user_id"] != user] == [row for row in rows(after) if row["user_id"] != user]
    return check


def actions(context):
    yield Action("read-unknown", "P01", "GET", "/api/users/me/preferences", 200,
                 headers=headers(0), actor_id=ACTOR,
                 assert_after=lambda before, after, response, mail: (
                     assertion(ACTOR, unchanged=True)(before, after, response, mail),
                     _require(json.loads(response["body"])["legacy_unknown"] == [1, {"keep": None}])
                 ))
    fixture = json.loads((ROOT / "tests/rust_contract/phase2/preferences.json").read_text())
    index = 0
    for case in fixture["vectors"]:
        # Raw syntax/redirect cases stay in the accepted component evidence. The
        # shared Action interface sends JSON, not arbitrary raw bytes/redirects.
        if case["id"].startswith("syntax-") or case["id"] == "empty-namespace":
            continue
        index += 1
        head = headers(index)
        success = case["python_status"] == 200
        yield Action(case["id"], "P02", "PUT", "/api/users/me/preferences/" + case["namespace"],
                     case["python_status"], body=case["value"], headers=head, actor_id=ACTOR,
                     outcome="committed" if success else "rolled-back",
                     assert_after=assertion(ACTOR, case["namespace"], case["value"], head["x-test-preference-clock"], not success))
    for offset, name in enumerate(("repeat-first", "repeat-second", "response-loss", "read-after-loss"), 100):
        head = headers(offset)
        lost = name == "response-loss"
        if lost:
            head["x-test-preference-response-loss"] = "true"
        read = name == "read-after-loss"
        yield Action(name, "P01" if read else "P02", "GET" if read else "PUT",
                     "/api/users/me/preferences" + ("" if read else "/tutorial"), 503 if lost else 200,
                     body=None if read else {"completed": True}, headers=head, actor_id=ACTOR,
                     outcome="ambiguous" if lost else "committed",
                     assert_after=assertion(ACTOR, None if read else "tutorial", {"completed": True}, head["x-test-preference-clock"], read))
    yield Action("database-rollback", "P02", "PUT", "/api/users/me/preferences/market_watch", 500,
                 body={"products": [], "portIds": ["rollback"]}, headers=headers(110), actor_id=ACTOR,
                 outcome="rolled-back", assert_after=assertion(ACTOR, unchanged=True))
    yield Action("other-isolated", "P01", "GET", "/api/users/me/preferences", 200,
                 headers=headers(111, OTHER), actor_id=OTHER,
                 assert_after=lambda before, after, response, mail: (
                     assertion(OTHER, unchanged=True)(before, after, response, mail),
                     _require(json.loads(response["body"]) == {})))


def _require(condition):
    assert condition


def races(context):
    # Both writers update the same existing row with identical values and clock.
    # Either SQL lock order must produce one row, never a duplicate. No claim is
    # made that a client barrier fixes SQL lock order. The accepted component run
    # separately observed both requests blocked on the authenticated user row.
    head = headers(200)
    actors = tuple(Action(name, "P02", "PUT", "/api/users/me/preferences/tutorial", 200,
                          body={"completed": False}, headers=head, actor_id=ACTOR) for name in ("same-user-a", "same-user-b"))
    def invariant(before, after, results):
        assertion(ACTOR, "tutorial", {"completed": False}, head["x-test-preference-clock"])(before, after, None, [])
        assert len(rows(before)) == len(rows(after))
        assert all(result[0]["status"] == 200 and result[1] is None for result in results.values())
    return [Race(actors=actors, invariant=invariant, repeats=3, seed=20261019)]


def preferences_scenario():
    return Scenario(name="preferences", group="preferences", setup=setup, servers=servers,
                    actions=actions, races=races,
                    build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
                           "--features", "contract-tests", "--example", "preferences_server"))
