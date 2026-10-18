"""Complete synthetic scenario; not evidence for a migrated CRUD route."""


def example_scenario():
    from scripts.rust_contract.phase2_scenarios import (
        Action,
        Race,
        Scenario,
        ScenarioError,
        Server,
    )

    def setup(connection):
        # The shared seed already contains this synthetic actor.
        connection.execute("SELECT id FROM users WHERE id='20000000-0000-4000-8000-000000000001'")

    def servers(context):
        return [Server((context.python_bin, "-m", "scripts.rust_contract.phase2_oracle", "--port", "{port}")),
                Server((str(context.root / "rs/target/debug/examples/phase2_transport"),))]

    def make_action(name, number, fault="none"):
        return Action(name=name, route_id="N02", method="POST", path="/__contract__/phase2/write",
            expected_status=503 if fault == "after-commit" else 200,
            body={"ids": [f"30000000-0000-4000-8000-{number:012d}", f"40000000-0000-4000-8000-{number:012d}"],
                  "now": "2026-10-18T00:00:00.000001+00:00", "referral_code": "VDX-ABC123", "fault": fault},
            outcome="rolled-back" if fault in {"between-writes", "before-commit"} else "ambiguous" if fault == "after-commit" else "committed",
            assert_after=assert_action)

    def assert_action(before, after, response, mail):
        result = __import__("json").loads(response["body"])
        rows = after["feedback_entries"]["rows"]
        ids = {row["id"] for row in rows.values()}
        expected = result["outcome"] != "rolled-back"
        if any((value in ids) != expected for value in result["ids"]) or mail:
            raise ScenarioError("synthetic unit missing, partial, replayed or sent mail")

    def actions(_context):
        return [make_action("committed", 1), make_action("rollback", 2, "between-writes"),
                make_action("response-loss", 3, "after-commit")]

    def invariant(before, after, _results):
        from scripts.rust_contract.phase2_state import compare_state
        if compare_state(before, after):
            raise ScenarioError("concurrent rollback changed state")

    def races(_context):
        return [Race(tuple(make_action(f"actor-{index}", 20 + index, "before-commit") for index in range(3)), invariant)]

    def mixed(_context):
        return [(0, make_action("python-mixed", 40)), (1, make_action("rust-mixed", 41))]

    def dispatch(context):
        import json
        import subprocess
        result = subprocess.run([context.python_bin, "-m", "tests.rust_contract.phase2.mixed_dispatch"],
            cwd=context.root, env={**context.environment, "PHASE2_DATABASE_URL": context.database_url},
            check=True, capture_output=True, text=True, timeout=60)
        return json.loads(result.stdout.strip().splitlines()[-1])

    def assert_mixed(state, _result):
        if len(state["feedback_entries"]["rows"]) != 4:
            raise ScenarioError("mixed writers lost rows")

    return Scenario(name="runner-example", group="all", setup=setup, servers=servers, actions=actions,
        build=("cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked", "-p", "verdaxis-read",
               "--features", "contract-tests", "--example", "phase2_transport"),
        races=races, mixed=mixed, dispatch=dispatch, assert_mixed=assert_mixed, route_qualification=False)
