"""Exact PD8 approved race difference. Ordinary actions use the shared comparator."""
import copy
import json
from dataclasses import dataclass

from scripts.rust_contract.phase2_scenarios import (
    Race,
    _compare_http,
    _request,
    run_barrier,
)
from scripts.rust_contract.phase2_seed import digest


@dataclass(frozen=True)
class PD8Race(Race):
    pass


def run_pd8(race, cells):
    from scripts.rust_contract.phase2_alerts import ORG
    vectors = json.loads((__import__("pathlib").Path(__file__).resolve().parents[2] / "tests/rust_contract/phase2/pd8-differences.json").read_text())
    assert race.seed in {vector["seed"] for vector in vectors["vectors"]}
    assert len(race.actors) == 2 and race.repeats == 1
    assert all(action.route_id == "A01" and action.method == "POST" and action.path == "/api/alerts" for action in race.actors)
    before = [cell.snapshot() for cell in cells]
    assert before[0] == before[1]
    results = [run_barrier(tuple(action.name for action in race.actors),
                          lambda name: _request(cell, next(a for a in race.actors if a.name == name)), seed=race.seed)
               for cell in cells]
    after = [cell.snapshot() for cell in cells]
    python, rust = results
    race.invariant(before[1], after[1], rust)
    assert sorted(value[0]["status"] for value in python.values()) == [201, 201]
    assert sorted(value[0]["status"] for value in rust.values()) == [201, 403], "PD8: Rust must admit one create and refuse the other"
    refusal = "Free tier limit of 5 active alerts reached. Delete an existing alert to create a new one."
    loser = next(name for name in rust if rust[name][0]["status"] == 403)
    for index, state in enumerate(after):
        assert sum(row["is_active"] for row in state["price_alerts"]["rows"].values() if row["org_id"] == ORG) == (6 if index == 0 else 5)
        for table in before[index]:
            if table != "price_alerts":
                assert state[table] == before[index][table], f"PD8 unrelated {table} changed"
        assert cells[index].mail() == []
    adjusted = copy.deepcopy(after[0])
    rejected_action = next(action for action in race.actors if action.name == loser)
    rejected_id = rejected_action.headers["x-alert-id"]
    rows = adjusted["price_alerts"]["rows"]
    key = next(key for key, row in rows.items() if row["id"] == rejected_id)
    del rows[key]
    assert adjusted == after[1], "PD8 only the refused candidate row may differ"
    for action in race.actors:
        p, r = python[action.name], rust[action.name]
        assert p[1] is None and r[1] is None
        if action.name == loser:
            assert json.loads(r[0]["body"]) == {"detail": refusal}
            # Compare headers exactly to the successful oracle response; only
            # status/body and the corresponding row are the approved difference.
            assert {k.lower():v for k,v in p[0]["headers"].items() if k.lower() not in {"date","server","content-length"}} == {k.lower():v for k,v in r[0]["headers"].items() if k.lower() not in {"date","server","content-length"}}
        else:
            _compare_http(action, [p[0], r[0]], [None, None])
    return [{"approved_difference":"PD8", "seed":race.seed,
             "actor_order":[a.name for a in race.actors], "python_statuses":{k:v[0]["status"] for k,v in python.items()},
             "rust_statuses":{k:v[0]["status"] for k,v in rust.items()},
             "active_counts":[6,5], "state_sha256":[digest(state) for state in after]}]
