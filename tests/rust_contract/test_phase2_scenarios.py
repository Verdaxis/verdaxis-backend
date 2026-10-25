"""Shared route scenarios must prove HTTP, committed state and side effects."""
import copy
import importlib
import json
import threading
import urllib.error
import urllib.request

import pytest


def runner_module():
    from pathlib import Path
    path = Path(__file__).resolve().parents[2] / "scripts/rust_contract/phase2_scenarios.py"
    assert path.exists(), "missing shared route-scenario runner"
    return importlib.import_module("scripts.rust_contract.phase2_scenarios")


def test_group_selection_never_substitutes_harness_smoke(monkeypatch):
    from types import SimpleNamespace
    module = runner_module()
    monkeypatch.setattr(module, "REGISTRY", {})
    for group in ("notifications", "all"):
        with pytest.raises(module.ScenarioError, match="registered route"):
            module.select_scenarios(group, None)
    fake = SimpleNamespace(name="fake-notifications", group="notifications", route_qualification=True)
    monkeypatch.setattr(module, "REGISTRY", {fake.name: fake})
    assert module.select_scenarios("notifications", None) == [fake]
    assert module.select_scenarios("all", None) == [fake]
    assert module.select_scenarios("all", [fake.name]) == [fake]
    with pytest.raises(module.ScenarioError, match="group"):
        module.select_scenarios("alerts", [fake.name])
    fake.route_qualification = False
    with pytest.raises(module.ScenarioError, match="registered route"):
        module.select_scenarios("all", None)
    assert module.select_scenarios("all", [fake.name]) == [fake]


def test_created_route_databases_follow_runtime_test_target_grammar(monkeypatch):
    import re
    from types import SimpleNamespace

    from scripts.rust_contract import phase2_routes, run_disposable

    created = []

    class Connection:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def execute(self, sql):
            if sql.startswith("CREATE DATABASE"):
                created.append(sql.split('"')[1])

    class StopBeforeServers(Exception):
        pass

    def stop(_context):
        raise StopBeforeServers

    monkeypatch.setattr(phase2_routes, "connect", lambda *_args, **_kwargs: Connection())
    monkeypatch.setattr(phase2_routes, "snapshot", lambda _connection: {})
    monkeypatch.setattr(run_disposable, "docker", lambda *_args: "")
    runner = SimpleNamespace(start_database=lambda: 49152, sha="a" * 40,
        urls=lambda _port: {}, migrate_and_seed=lambda _env: None,
        container="owned-test", python_bin="python")
    scenario = SimpleNamespace(setup=lambda _connection: None, build=(), servers=stop)
    with pytest.raises(StopBeforeServers):
        phase2_routes.run_registered(runner, SimpleNamespace(phase2_races=False), [scenario])
    assert len(created) == 4
    for database in created:
        assert re.fullmatch(r"[a-z][a-z0-9_]*_test", database), database
    assert len(set(created)) == len(created)


class Cell:
    def __init__(self, *, extra=False, lost=False):
        self.calls = 0
        self.rows = {}
        self.extra = extra
        self.lost = lost
        self.sends = []

    def snapshot(self):
        return {"users": {"columns": ["id"], "rows": {}},
                "feedback_entries": {"columns": ["id"], "rows": copy.deepcopy(self.rows)}}

    def request(self, action):
        self.calls += 1
        if action.outcome != "rolled-back":
            self.rows[action.name] = {"id": action.name}
        if self.extra:
            self.rows["extra"] = {"id": "extra"}
        if self.lost:
            raise OSError("response lost")
        return {"status": 200, "headers": {"content-type": "application/json"}, "body": b'{"ok":true}'}

    def mail(self):
        return copy.deepcopy(self.sends)


def action(module, name="write", **kwargs):
    return module.Action(name=name, route_id="N02", method="POST", path="/test", expected_status=200, **kwargs)


def test_approved_http_difference_requires_named_rule_and_exact_pair():
    module=runner_module()
    left={"status":200,"headers":{"content-type":"application/json"},"body":b'{"count":1}'}
    right={"status":200,"headers":{"content-type":"application/json"},"body":b'{"count":0}'}
    checked=[]
    def approved(pair):
        assert pair==[left,right]
        checked.append(True)
    item=action(module,approved_difference="PD4-G",assert_http_difference=approved)
    module._compare_http(item,[left,right],[None,None])
    assert checked==[True]
    with pytest.raises(module.ScenarioError):
        module._compare_http(action(module,approved_difference="PD4-G"),[left,right],[None,None])


def test_combined_approved_difference_names_must_all_be_reviewed():
    module=runner_module()
    left={"status":200,"headers":{"content-type":"application/json"},"body":b'{"count":1}'}
    right={"status":200,"headers":{"content-type":"application/json"},"body":b'{"count":0}'}
    checked=[]
    def approved(pair):
        checked.append(pair)
    combined=action(module,approved_difference="PD2+PD3",assert_http_difference=approved)
    module._compare_http(combined,[left,right],[None,None])
    assert checked==[[left,right]]
    for unreviewed in ("PD2+PD9","PD9","PD2+"):
        with pytest.raises(module.ScenarioError):
            module._compare_http(action(module,approved_difference=unreviewed,assert_http_difference=approved),[left,right],[None,None])
    assert len(checked)==1


def test_approved_difference_does_not_parse_a_non_json_response():
    module=runner_module()
    left={"status":500,"headers":{"content-type":"text/plain"},"body":b"Internal Server Error"}
    right={"status":404,"headers":{"content-type":"application/json"},"body":b'{"detail":"Order not found"}'}
    seen=[]
    item=action(module,approved_difference="PD1+PD6",assert_http_difference=lambda pair: seen.append(pair))
    module._compare_http(item,[left,right],[None,None])
    assert seen==[[left,right]]


def test_invalid_bearer_actions_pin_the_reviewed_credential_envelope():
    """AUTH-ENVELOPE/-11: Rust answers an invalid bearer with the coded detail;
    Python keeps its legacy string (decisions 47 and 113). Every bad-bearer
    action must name the rule and pin both wires exactly, so the twins cannot
    silently drift while the envelopes differ."""
    from pathlib import Path
    from types import SimpleNamespace

    module = runner_module()
    root = Path(__file__).resolve().parents[2]
    context = SimpleNamespace(environment={"JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars"},
                              root=root, python_bin="python")
    expected = {
        "watchlist-http-reads": ["bad-bearer-before-uuid"],
        "watchlist-http-reads-gate-off": ["bad-bearer-before-uuid"],
        "watchlist-containers": ["create-bad-bearer", "create-bad-bearer-before-validation"],
        "watchlist-targets": ["bad-bearer-before-validation", "delete-bad-bearer"],
        "watchlist-targets-gate-off": ["bad-bearer-before-validation", "delete-bad-bearer"],
    }
    for name, bad_bearers in expected.items():
        scenario = module.REGISTRY[name]
        marked = {action.name: action for action in scenario.actions(context) if action.name in bad_bearers}
        assert set(marked) == set(bad_bearers), name
        for action_name, item in marked.items():
            assert item.approved_difference == "AUTH-REFUSAL", (name, action_name)
            assert item.assert_http_difference is not None, (name, action_name)


def test_invalid_bearer_pair_pins_both_reviewed_envelopes():
    module = runner_module()
    check = module.invalid_bearer_pair()
    python = {"status": 401, "headers": {"www-authenticate": "Bearer", "content-type": "application/json",
                                         "content-length": "42"},
              "body": b'{"detail":"Could not validate credentials"}'}
    rust = {"status": 401, "headers": {"www-authenticate": "Bearer", "content-type": "application/json"},
            "body": b'{"detail":{"code":"AUTH_CREDENTIALS_INVALID","message":"Incorrect email or password"}}'}
    check([python, rust])
    # Wording is free; a changed message must not fail the reviewed pair.
    check([python, {**rust, "body": b'{"detail":{"code":"AUTH_CREDENTIALS_INVALID","message":"Sign in"}}'}])
    with pytest.raises(module.ScenarioError, match="headers differ"):
        check([python, {**rust, "headers": {**rust["headers"], "cache-control": "no-store"}}])
    with pytest.raises(module.ScenarioError, match="must be JSON"):
        check([python, {**rust, "headers": {"www-authenticate": "Bearer", "content-type": "text/plain"}}])
    with pytest.raises(module.ScenarioError, match="code differs"):
        check([python, {**rust, "body": b'{"detail":{"code":"AUTH_INVALID","message":"x"}}'}])
    with pytest.raises(module.ScenarioError, match="Python invalid-bearer body"):
        check([{**python, "body": b'{"detail":"Not authenticated"}'}, rust])
    with pytest.raises(module.ScenarioError, match="must be refused with 401"):
        check([python, {**rust, "status": 403}])
    with pytest.raises(module.ScenarioError, match="challenge"):
        check([python, {**rust, "headers": {}}])
    with pytest.raises(module.ScenarioError, match="reviewed object"):
        check([python, {**rust, "body": b'{"detail":{"code":"AUTH_CREDENTIALS_INVALID"}}'}])
    with pytest.raises(module.ScenarioError, match="non-empty string"):
        check([python, {**rust, "body": b'{"detail":{"code":"AUTH_CREDENTIALS_INVALID","message":""}}'}])
    assert "AUTH-REFUSAL" in module.APPROVED_DIFFERENCES


def test_state_difference_needs_a_named_rule_and_is_not_silent():
    module=runner_module()

    def cells_where_python_stores_one_extra_row():
        python,rust=Cell(),Cell()
        original=python.request
        def stores_extra(item):
            python.rows["stored"]={"id":"stored"}
            return original(item)
        python.request=stores_extra
        return [python,rust]

    def pinned(before,after):
        assert before[0]==before[1]
        assert set(after[0]["feedback_entries"]["rows"])=={"stored","write"}
        assert set(after[1]["feedback_entries"]["rows"])=={"write"}
        seen.append(True)

    seen=[]
    with pytest.raises(module.ScenarioError,match="named approved difference"):
        module.run_actions([action(module,assert_state_difference=pinned)],cells_where_python_stores_one_extra_row())
    with pytest.raises(module.ScenarioError,match="committed state differs"):
        module.run_actions([action(module)],cells_where_python_stores_one_extra_row())
    assert seen==[]
    module.run_actions([action(module,approved_difference="PD1",assert_http_difference=lambda pair: None,
                               assert_state_difference=pinned)],cells_where_python_stores_one_extra_row())
    assert seen==[True]


def test_equal_http_does_not_hide_extra_committed_row():
    module = runner_module()
    with pytest.raises(module.ScenarioError, match="state"):
        module.run_actions([action(module)], [Cell(), Cell(extra=True)])


def test_rollback_and_lost_response_are_observed_without_replay():
    module = runner_module()
    cells = [Cell(), Cell()]
    evidence = module.run_actions([action(module, outcome="rolled-back")], cells)
    assert evidence[0]["outcome"] == "rolled-back"
    assert all(cell.calls == 1 and not cell.rows for cell in cells)
    cells = [Cell(lost=True), Cell(lost=True)]
    evidence = module.run_actions([action(module, outcome="ambiguous", allow_transport_error=True)], cells)
    assert all(cell.calls == 1 and len(cell.rows) == 1 for cell in cells)
    assert evidence[0]["transport_errors"] == ["OSError", "OSError"]
    with pytest.raises(module.ScenarioError, match="rollback"):
        module.run_actions([action(module, outcome="rolled-back")], [Cell(extra=True), Cell(extra=True)])


def test_expected_status_and_mail_are_not_only_pairwise_checks():
    module = runner_module()
    item = module.Action(name="bad", route_id="N02", method="POST", path="/test", expected_status=201)
    with pytest.raises(module.ScenarioError, match="status"):
        module.run_actions([item], [Cell(), Cell()])
    cells = [Cell(), Cell()]
    cells[1].sends.append({"payload": {"to": ["synthetic@example.invalid"]}})
    with pytest.raises(module.ScenarioError, match="mail"):
        module.run_actions([action(module)], cells)


def test_barrier_races_repeat_seed_and_keep_actor_identity():
    module = runner_module()
    seen = []
    lock = threading.Lock()
    rendezvous = threading.Barrier(3, timeout=5)

    def invoke(actor):
        rendezvous.wait()
        with lock:
            seen.append(actor)
        return actor

    first = module.run_barrier(["one", "two", "three"], invoke, seed=41)
    assert first == {"one": "one", "two": "two", "three": "three"}
    assert set(seen) == set(first)
    assert module.actor_order(["one", "two", "three"], 41) == module.actor_order(["one", "two", "three"], 41)


def test_mixed_schedule_calls_each_writer_once_before_dispatch():
    module = runner_module()
    calls = []
    cells = [Cell(), Cell()]
    for index, cell in enumerate(cells):
        original = cell.request
        def request(item, index=index, original=original):
            calls.append(index)
            return original(item)
        cell.request = request
    def dispatch():
        assert calls == [0, 1]
        return {"delivered": 2}
    result = module.run_mixed([(0, action(module, "python")), (1, action(module, "rust"))], cells, dispatch)
    assert result["dispatcher"] == {"delivered": 2}
    assert all(cell.calls == 1 for cell in cells)


def test_race_mode_repeats_and_runs_each_runtime_invariant():
    module = runner_module()
    observed = []
    def invariant(before, after, results):
        assert before == after
        assert set(results) == {"one", "two"}
        observed.append(True)
    race = module.Race((action(module, "one", outcome="rolled-back"),
                        action(module, "two", outcome="rolled-back")), invariant, repeats=3, seed=7)
    evidence = module.run_race(race, [Cell(), Cell()])
    assert len(observed) == 6
    assert [row["seed"] for row in evidence] == [7, 8, 9]


class FirstWins(Cell):
    """One named actor is created; every other request conflicts."""

    def __init__(self, winner):
        super().__init__()
        self.winner = winner

    def request(self, action):
        self.calls += 1
        if action.name == self.winner:
            # Both stacks commit the same row, so only the actor-specific
            # response can make an uncontested comparison fail.
            self.rows["created"] = {"id": "created"}
            return {"status": 201, "headers": {"content-type": "application/json"}, "body": b'{"ok":true}'}
        return {"status": 409, "headers": {"content-type": "application/json"}, "body": b'{"detail":"duplicate"}'}


def test_contested_race_compares_response_sets_not_actor_names():
    module = runner_module()
    seen = []

    def invariant(before, after, results):
        statuses = sorted(response["status"] for response, _error in results.values())
        assert statuses == [201, 409]
        assert len(after["feedback_entries"]["rows"]) == 1
        seen.append(statuses)

    actors = tuple(action(module, name, accepted_statuses=(201, 409)) for name in ("first", "second"))
    race = module.Race(actors, invariant, repeats=1, seed=5, contested=True)
    cells = [FirstWins("first"), FirstWins("second")]
    module.run_race(race, cells)
    assert len(seen) == 2


def test_uncontested_race_still_pairs_responses_by_actor_name():
    module = runner_module()
    actors = tuple(action(module, name, accepted_statuses=(201, 409)) for name in ("first", "second"))
    race = module.Race(actors, lambda *_args: None, repeats=1, seed=5)
    # Actor identity is part of an uncontested comparison: a status outside the
    # accepted set, or a different response per actor, must still fail.
    with pytest.raises(module.ScenarioError):
        module.run_race(race, [FirstWins("first"), FirstWins("second")])


def test_contested_race_rejects_a_status_no_actor_may_receive():
    module = runner_module()
    actors = tuple(action(module, name, accepted_statuses=(201,)) for name in ("first", "second"))
    race = module.Race(actors, lambda *_args: None, repeats=1, seed=5, contested=True)
    with pytest.raises(module.ScenarioError, match="unexpected HTTP status 409"):
        module.run_race(race, [FirstWins("first"), FirstWins("second")])


def test_watchlist_inventory_names_exactly_the_registered_actions():
    from pathlib import Path
    from types import SimpleNamespace

    module = runner_module()
    inventory = json.loads((Path(__file__).parent / "phase2/scenarios.json").read_text())["scenarios"]
    context = SimpleNamespace(environment={"JWT_SECRET": "test-secret-key-for-testing-minimum-32-chars"},
                              root=Path("."), python_bin="python")
    names = [name for name, scenario in module.REGISTRY.items() if scenario.group == "watchlists"]
    assert names
    for name in names:
        listed = [row for row in inventory if row["id"] == name]
        assert len(listed) == 1, name
        registered = [action.name for action in module.REGISTRY[name].actions(context)]
        assert listed[0]["actions"] == registered, name
        assert len(set(registered)) == len(registered), name
        scheduled = module.REGISTRY[name].races
        assert bool(listed[0].get("races")) == (scheduled is not None), name
        if scheduled is not None:
            assert listed[0]["races"] == [race.actors[0].name.rsplit("-", 1)[0] for race in scheduled(context)], name


def test_oracle_refuses_external_mail_endpoint(monkeypatch):
    from scripts.rust_contract.phase2_oracle import main
    monkeypatch.setenv("ENVIRONMENT", "test")
    monkeypatch.setenv("DISPOSABLE_TEST_TOKEN", "x" * 32)
    monkeypatch.setenv("PHASE2_MAIL_URL", "https://api.resend.com/emails")
    with pytest.raises(SystemExit, match="loopback"):
        main()


def test_loopback_mail_records_failures_and_closes():
    module = runner_module()
    from scripts.rust_contract.phase2_mail import MailStub
    payload = {"to": ["recipient@example.invalid"], "from": "sender@example.invalid", "subject": "test", "html": "escaped"}
    with MailStub() as stub:
        assert stub.url.startswith("http://127.0.0.1:")
        stub.respond(status=503)
        request = urllib.request.Request(stub.url + "/python/emails", data=json.dumps(payload).encode(), headers={"content-type": "application/json"})
        with pytest.raises(urllib.error.HTTPError) as error:
            urllib.request.urlopen(request, timeout=5)
        assert error.value.code == 503
        assert stub.ledger("python")[0]["payload"] == payload
        assert stub.ledger("python")[0]["status"] == 503
        assert stub.ledger("rust") == []
        stub.respond(status=201)
        with urllib.request.urlopen(request, timeout=5) as response:
            assert response.status == 201
        assert len(stub.ledger("python")) == 2
    assert not stub.thread.is_alive()
