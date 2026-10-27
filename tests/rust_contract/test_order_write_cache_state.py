"""Order-write invariants retain exact cache rows and table metadata."""
import copy
import importlib
import json

import pytest

OLD_KEY = json.dumps(["trading", 7])
NEW_KEY = json.dumps(["trading", 8])
DOMAIN_KEY = json.dumps(["trading"])


def action_state(name="atomic-outbox-receipt"):
    importlib.import_module("scripts.rust_contract.phase2_scenarios")
    module = importlib.import_module("scripts.rust_contract.phase2_order_write")
    action = next(item for item in module.order_write_scenario().actions(None) if item.name == name)
    identifier = action.body["id"]
    replayed = name in {"receipt-replay", "explicit-replay-after-loss"}
    before = {
        "public_read_cache_generations": {"columns": ["domain", "generation"], "rows": {
            DOMAIN_KEY: {"domain": "trading", "generation": 7}}},
        "public_read_cache_events": {"columns": ["domain", "generation"], "rows": {
            OLD_KEY: {"domain": "trading", "generation": 7}}},
        "market_event_outbox": {"columns": ["id"], "rows": {}},
        "market_command_results": {"columns": ["id"], "rows": {}},
    }
    if replayed:
        for table in ("market_event_outbox", "market_command_results"):
            before[table]["rows"][identifier] = {"id": identifier}
    after = copy.deepcopy(before)
    if not replayed:
        after["public_read_cache_generations"]["rows"][DOMAIN_KEY]["generation"] = 8
        after["public_read_cache_events"]["rows"][NEW_KEY] = {"domain": "trading", "generation": 8}
        for table in ("market_event_outbox", "market_command_results"):
            after[table]["rows"][identifier] = {"id": identifier}
    response = {"body": json.dumps({"id": identifier, "status": 200, "replayed": replayed,
                                     "attempts": 1, "lock_observed": False}).encode()}
    return action, before, after, response


def test_committed_cache_change_is_exact():
    action, before, after, response = action_state()
    original = copy.deepcopy(before)
    action.assert_after(before, after, response, [])
    assert before == original


@pytest.mark.parametrize("damage", [
    "new-event-key", "new-event-value", "old-event-deletion", "old-event-mutation",
    "different-domain", "generation-columns", "event-columns", "extra-generation-row",
])
def test_committed_cache_change_rejects_metadata_damage(damage):
    action, before, after, response = action_state()
    events = after["public_read_cache_events"]["rows"]
    if damage == "new-event-key":
        events[json.dumps(["trading", 99])] = events.pop(NEW_KEY)
    elif damage == "new-event-value":
        events[NEW_KEY]["generation"] = 99
    elif damage == "old-event-deletion":
        del events[OLD_KEY]
        events[json.dumps(["trading", 99])] = {"domain": "trading", "generation": 99}
    elif damage == "old-event-mutation":
        events[OLD_KEY]["generation"] = 99
    elif damage == "different-domain":
        after["public_read_cache_generations"]["rows"][DOMAIN_KEY]["domain"] = "other"
        events[NEW_KEY]["domain"] = "other"
    elif damage == "generation-columns":
        after["public_read_cache_generations"]["columns"].reverse()
    elif damage == "event-columns":
        after["public_read_cache_events"]["columns"].reverse()
    else:
        after["public_read_cache_generations"]["rows"][json.dumps(["other"])] = {
            "domain": "other", "generation": 99}
    with pytest.raises(AssertionError):
        action.assert_after(before, after, response, [])


@pytest.mark.parametrize("name", ["receipt-replay", "explicit-replay-after-loss"])
def test_replay_rejects_existing_cache_event_mutation(name):
    action, before, after, response = action_state(name)
    action.assert_after(before, after, response, [])
    after["public_read_cache_events"]["rows"][OLD_KEY]["generation"] = 99
    with pytest.raises(AssertionError):
        action.assert_after(before, after, response, [])
