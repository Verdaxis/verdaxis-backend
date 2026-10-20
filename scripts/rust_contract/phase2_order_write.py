"""Task 20 database schedules. These qualify the boundary, not a market route."""
import json


def order_write_scenario():
    from scripts.rust_contract.phase2_scenarios import Action, Race, Scenario, Server

    def setup(connection):
        connection.execute("SELECT id FROM users WHERE id='20000000-0000-4000-8000-000000000001'")

    def servers(context):
        return [Server((context.python_bin, "-m", "scripts.rust_contract.phase2_order_write_oracle")),
                Server((str(context.root / "rs/target/debug/examples/order_write"),))]

    def action(name, number, mode="commit", replayed=False):
        identifier = f"71000000-0000-4000-8000-{number:012d}"
        def check(before, after, response, mail):
            result = json.loads(response["body"])
            assert result == {"id":identifier,"status":500 if mode == "rollback" else 503 if mode == "lost-response" else 503 if mode=="retry-40P01-bound" else 409 if mode=="timeout" else 200,
                              "replayed":replayed,"attempts":2 if mode.startswith("retry-") else 1,"lock_observed":mode=="ordered"}
            assert not mail
            outbox = [r for r in after["market_event_outbox"]["rows"].values() if r["id"]==identifier]
            receipts = [r for r in after["market_command_results"]["rows"].values() if r["id"]==identifier]
            expected = 0 if mode in {"rollback", "timeout", "retry-40P01-bound"} else 1
            assert len(outbox) == len(receipts) == expected
            for table in before:
                if table not in {"market_event_outbox", "market_command_results"}:
                    assert before[table] == after[table], table
        return Action(name, "N02", "POST", "/__contract__/order-write", 200,
            body={"id":identifier,"mode":mode}, outcome="rolled-back" if mode in {"rollback","timeout","retry-40P01-bound"} else "ambiguous" if mode=="lost-response" else "committed",
            assert_after=check)

    def actions(context):
        return [action("rollback-after-outbox",1,"rollback"), action("atomic-outbox-receipt",2),
                action("receipt-replay",2,replayed=True), action("slice-timeout",3,"timeout"),
                action("database-lock-order",4,"ordered"), action("lost-response-no-replay",5,"lost-response"),
                action("explicit-replay-after-loss",5,replayed=True), action("retry-serialization",6,"retry-40001"),
                action("deadlock-attempt-bound",7,"retry-40P01-bound")]

    def invariant(before, after, results):
        assert before == after
        for response, error in results.values():
            assert error is None
            result = json.loads(response["body"])
            assert result["status"] == 500 and result["attempts"] == 1
            assert result["lock_observed"] is False

    def races(context):
        return [Race(tuple(action(f"rollback-actor-{i}",20,"rollback") for i in range(3)), invariant,
                     repeats=3, seed=20261019)]

    def mixed(context):
        return [(0,action("python-boundary",40)), (1,action("rust-boundary",41))]

    def dispatch(context):
        import subprocess
        result = subprocess.run([context.python_bin,"-m","tests.rust_contract.phase2.mixed_dispatch"],
            cwd=context.root, env={**context.environment,"PHASE2_DATABASE_URL":context.database_url},
            check=True,capture_output=True,text=True,timeout=60)
        return json.loads(result.stdout.strip().splitlines()[-1])

    def assert_mixed(state, result):
        assert len(state["market_command_results"]["rows"]) == 2
        assert len(state["market_event_outbox"]["rows"]) == 2

    return Scenario("order-write-postgres","order-write-boundary",setup,servers,actions,
        build=("cargo","build","--manifest-path","rs/Cargo.toml","--locked","-p","verdaxis-read",
               "--features","contract-tests","--example","order_write"),
        races=races,mixed=mixed,dispatch=dispatch,assert_mixed=assert_mixed)
