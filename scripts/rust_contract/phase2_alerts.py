"""Task 16 real alert route schedules. No container/process lifecycle ownership."""
import json
import subprocess
from datetime import UTC, datetime

ORG = "10000000-0000-4000-8000-000000000001"
FOREIGN = "10000000-0000-4000-8000-000000000002"
PRODUCT = "30000000-0000-4000-8000-000000000001"
POINT = "40000000-0000-4000-8000-000000000001"
ACTORS = [f"20000000-0000-4000-8000-{i:012d}" for i in range(1, 5)]


def alert_id(number):
    return f"50000000-0000-4000-8000-{number:012d}"


def setup(connection):
    connection.execute("INSERT INTO organizations(id,name,type) VALUES (%s,'Foreign alert fixture','FUEL_BUYER')", (FOREIGN,))
    for actor, org in zip(ACTORS[1:], (ORG, FOREIGN, None)):
        connection.execute("INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) VALUES (%s,%s,'unused','BUYER','APPROVED',%s,true,false)", (actor, f"{actor}@example.test", org))
    connection.execute("INSERT INTO products(id,name,fuel_type,fuel_grade,is_active) VALUES (%s,'Alert runner legacy','TEST','TEST',false)", (PRODUCT,))
    connection.execute("INSERT INTO delivery_points(id,name,region,is_active) VALUES (%s,'Alert runner point','TEST',false)", (POINT,))
    for i in range(1, 5):
        connection.execute("INSERT INTO price_alerts(id,org_id,product_id,direction,threshold_usd,is_active,created_at) VALUES (%s,%s,%s,'above',10,true,'2026-10-18T00:00:00Z')", (alert_id(i), ORG, PRODUCT))


def servers(context):
    from scripts.rust_contract.phase2_scenarios import Server
    return [Server((context.python_bin, "-m", "scripts.rust_contract.phase2_alert_oracle")),
            Server((str(context.root / "rs/target/debug/examples/alerts_contract"),))]


def action(name, method="POST", number=10, actor=0, status=201, body=None, fault=None, barrier=None):
    import jwt

    from scripts.rust_contract.phase2_scenarios import Action
    now = int(datetime.now(UTC).timestamp())
    token = jwt.encode({"sub": ACTORS[actor], "type": "access", "iss": "verdaxis-test-api", "aud": "verdaxis-test-web", "iat": now, "exp": now + 3600}, "test-secret-key-for-testing-minimum-32-chars", algorithm="HS256")
    headers = {"authorization": f"Bearer {token}", "x-request-id": name,
               "x-alert-id": alert_id(number), "x-alert-clock": f"2026-10-18T01:00:{number % 60:02d}+00:00"}
    if fault:
        headers["x-alert-fault"] = fault
    if barrier:
        headers["x-alert-barrier"] = barrier
    path = "/api/alerts" + (f"/{alert_id(number)}" if method == "DELETE" else "")
    if body is None and method == "POST":
        body = {"product_id": PRODUCT, "delivery_point_id": POINT, "direction": "above", "threshold_usd": "12.345"}
    expected_delta = 1 if method == "POST" and status in (201, 503) else -1 if method == "DELETE" and status == 204 else 0

    def check(before, after, response, mail):
        assert not mail, "alert CRUD sent email"
        old, new = before["price_alerts"]["rows"], after["price_alerts"]["rows"]
        assert len(new) - len(old) == expected_delta, "missing/partial/replayed alert unit"
        for table in before:
            if table != "price_alerts":
                assert before[table] == after[table], f"unrelated {table} changed"
        if expected_delta == 1:
            row = next(row for row in new.values() if row["id"] == alert_id(number))
            assert row["org_id"] == ORG and row["threshold_usd"] == 12.35 and row["is_active"] is True
    return Action(name, "A01" if method == "POST" else "A02" if method == "GET" else "A03", method, path, status,
                  body=body, headers=headers, actor_id=ACTORS[actor], assert_after=check,
                  outcome="rolled-back" if fault == "before-commit" else "ambiguous" if fault == "after-commit" else "committed")


def actions(_context):
    return [action("list-all", "GET", status=200), action("foreign-delete", "DELETE", 1, 2, 403),
            action("missing-delete", "DELETE", 999, status=404),
            action("no-org-get", "GET", actor=3, status=400),
            action("no-org-delete", "DELETE", 1, 3, 400),
            action("no-org-create", actor=3, status=400),
            action("rollback", number=11, status=500, fault="before-commit"),
            action("lost-response", number=12, status=503, fault="after-commit"),
            action("cap-reached", number=13, status=403),
            action("same-org-delete", "DELETE", 12, 1, 204),
            action("repeat-delete", "DELETE", 12, 1, 404)]


def races(_context):
    from scripts.rust_contract.phase2_scenarios import Race
    def invariant(before, after, results):
        assert len(after["price_alerts"]["rows"]) == len(before["price_alerts"]["rows"]) + 2
        assert all(result[0]["status"] == 201 for result in results.values())
        # Both SQL counts observe four before either INSERT. Source cap race is
        # explicitly reproduced, not silently repaired by a Rust-only org lock.
        assert sum(row["is_active"] for row in after["price_alerts"]["rows"].values()) == 6
    return [Race((action("cap-owner", number=20, barrier="cap-count"),
                  action("cap-member", number=21, actor=1, barrier="cap-count")), invariant, repeats=1, seed=20261018)]


def mixed(_context):
    return [(0, action("python-mixed-create", number=40)),
            (0, action("python-mixed-delete", "DELETE", 40, status=204)),
            (1, action("rust-mixed-create", number=41))]


def dispatch(context):
    result = subprocess.run([context.python_bin, "-m", "scripts.rust_contract.phase2_alert_dispatch"],
        cwd=context.root, env={**context.environment, "PHASE2_DATABASE_URL": context.database_url},
        capture_output=True, text=True, check=True, timeout=60)
    return json.loads(result.stdout.strip().splitlines()[-1])


def assert_mixed(state, result):
    assert result["dispatcher"]["delivered"] == 5
    assert all(not row["is_active"] and row["triggered_at"] for row in state["price_alerts"]["rows"].values())


def alerts_scenario(reverse=False):
    from scripts.rust_contract.phase2_scenarios import Scenario
    def schedules(context):
        values = races(context)
        if not reverse:
            return values
        from dataclasses import replace
        return [replace(race, actors=tuple(reversed(race.actors)), seed=20261019) for race in values]
    return Scenario("alerts-crud-reverse" if reverse else "alerts-crud", "alerts", setup, servers, actions,
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read", "--features", "contract-tests", "--example", "alerts_contract"),
        races=schedules, mixed=mixed, dispatch=dispatch, assert_mixed=assert_mixed)
