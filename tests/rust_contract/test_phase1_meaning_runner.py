"""Successor recording comparison requires explicit reviewed evidence."""
import json

import pytest

from scripts.rust_contract import compare
from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256


def recording(body):
    return [{"id": "R06-tape", "request": {"method": "GET", "path": "/api/trade-tape"},
             "exchange": compare.capture(200, {"content-type": "application/json"}, body), "raw_body": body}]


def test_successor_requires_reviewed_profile_and_complete_requests():
    entries = recording(b'{"items":[],"total":0,"market_hours":true}')
    assert compare.recording_differences(entries, [], profile="phase1-meaning-v1", rules_sha256=PHASE1_RULES_SHA256)
    with pytest.raises(compare.ContractError):
        compare.recording_differences(entries, entries, profile="phase1-meaning-v1", rules_sha256="")


def test_successor_does_not_fall_back_to_bytes():
    left = recording(b'{"items":[],"total":0,"market_hours":true}')
    right = recording(b'{ "market_hours":true,"total":0.0,"items":[] }')
    assert compare.recording_differences(left, right, profile="phase1-meaning-v1", rules_sha256=PHASE1_RULES_SHA256) == []
    assert compare.recording_differences(left, right)


def test_successor_corpus_keeps_every_request_in_both_kyc_states():
    import json
    from pathlib import Path
    root = Path(__file__).parent
    groups = json.loads((root / "cases.json").read_text())["groups"]
    for mode in ("enabled", "disabled"):
        paths = list((root / "phase1_meaning/corpus" / mode).glob("*.json"))
        assert {path.stem for path in paths} == set(groups)
        for path in paths:
            document = json.loads(path.read_text())
            assert document["profile"] == "phase1-meaning-v1"
            assert document["rules_sha256"] == PHASE1_RULES_SHA256
            ids = [entry["id"].removesuffix("--kyc-off") for entry in document["entries"]]
            assert ids == [case["id"] for case in groups[path.stem]]
            for entry in document["entries"]:
                assert entry["raw_body_base64"] is not None


# Three trades confirmed at the same instant: Python's raw page order ties, the
# compared order is the canonical full-UUID order.
TAPE = {"id": "R06-trade-tape", "method": "GET", "path": "/api/trade-tape"}
PRICES = {"id": "R12-prices", "method": "GET", "path": "/api/prices"}
CANONICAL_TAPE_IDS = ["c0000000-tape", "b0000000-tape", "a0000000-tape"]


def tape_page(ids):
    items = ",".join(
        '{"id":"%s","confirmed_at":"2026-10-15T12:00:00Z"}' % trade_id for trade_id in ids
    )
    return ('{"items":[' + items + '],"total":%d,"market_hours":true}' % len(ids)).encode()


def prices_page(last_price):
    summary = '{"fuel_type":"VLSFO","last_price":"%s"}' % last_price
    return ('{"summaries":[' + summary + '],"generated_at":"2026-10-15T12:00:00Z"}').encode()


def recorded(case, body):
    return {"id": case["id"], "request": dict(case, query=""),
            "exchange": compare.capture(200, {"content-type": "application/json"}, body),
            "raw_body": body}


async def canonical_tape_page(request, response):
    """The decision-85 canonical page: same rows, full-UUID tie order."""
    return {**response, "body": tape_page(CANONICAL_TAPE_IDS)}, {"rule_version": "decision85-v1"}


@pytest.fixture
def simulation(monkeypatch, tmp_path):
    from scripts.rust_contract import meaning_runner, simulate

    monkeypatch.setattr(meaning_runner, "canonical_response", canonical_tape_page)
    instance = simulate.Simulation("small", output_dir=tmp_path, groups=["prices"])
    monkeypatch.setattr(
        instance,
        "start_environment",
        lambda group: {
            "base_env": {
                "ENVIRONMENT": "test",
                "RUST_CONTRACT_PROFILE": "phase1-meaning-v1",
                "MEANING_RULES_SHA256": PHASE1_RULES_SHA256,
            },
            "python_url": "http://127.0.0.1:9",
            "rust_url": "http://127.0.0.1:9",
        },
    )
    monkeypatch.setattr(instance, "_seed_workload", lambda env: None)
    monkeypatch.setattr(instance, "stop_environment", lambda: None)
    return instance


def canned_record(monkeypatch, recordings):
    """Serve one prepared recording per record() call, in order."""
    from scripts.rust_contract import record

    calls = iter(recordings)
    monkeypatch.setattr(record, "record", lambda url, token, cases, **kwargs: next(calls))


def test_raw_tie_order_in_both_python_recordings_still_matches(simulation, monkeypatch):
    raw_ties = tape_page(list(reversed(CANONICAL_TAPE_IDS)))
    other_raw_ties = tape_page([CANONICAL_TAPE_IDS[1], CANONICAL_TAPE_IDS[2], CANONICAL_TAPE_IDS[0]])
    canned_record(monkeypatch, [
        [recorded(TAPE, raw_ties)],                # first Python recording, raw
        [recorded(TAPE, tape_page(CANONICAL_TAPE_IDS))],  # Rust keeps the tie order
        [recorded(TAPE, other_raw_ties)],          # third Python recording, raw
    ])
    outcome = simulation._run_group("prices", [TAPE])
    assert outcome["outcome"] == "match"
    assert outcome["differences"] == 0


def test_changed_non_tape_response_keeps_the_group_inconclusive(simulation, monkeypatch):
    stable_tape = [recorded(TAPE, tape_page(list(reversed(CANONICAL_TAPE_IDS))))]
    unchanged = [recorded(PRICES, prices_page("600.00"))]
    changed = [recorded(PRICES, prices_page("601.00"))]
    canned_record(monkeypatch, [
        stable_tape + unchanged,
        [recorded(TAPE, tape_page(CANONICAL_TAPE_IDS))] + unchanged,
        stable_tape + changed,
    ])
    outcome = simulation._run_group("prices", [TAPE, PRICES])
    assert outcome["outcome"] == "inconclusive"
    disagreed = outcome.get("cases_disagreed") or []
    assert "R12-prices" in disagreed
    assert "R06-trade-tape" not in disagreed
    assert outcome.get("paths")


def test_simulation_report_records_profile_rules_and_disagreed_cases(simulation, monkeypatch, tmp_path):
    from scripts.rust_contract import compare as compare_module
    from scripts.rust_contract import simulate

    monkeypatch.setenv("RUST_CONTRACT_PROFILE", "phase1-meaning-v1")
    monkeypatch.setenv("MEANING_RULES_SHA256", PHASE1_RULES_SHA256)
    stable_tape = [recorded(TAPE, tape_page(list(reversed(CANONICAL_TAPE_IDS))))]
    unchanged = [recorded(PRICES, prices_page("600.00"))]
    changed = [recorded(PRICES, prices_page("601.00"))]
    canned_record(monkeypatch, [
        stable_tape + unchanged,
        [recorded(TAPE, tape_page(CANONICAL_TAPE_IDS))] + unchanged,
        stable_tape + changed,
    ])
    monkeypatch.setattr(simulation, "build_command", lambda: ["true"])
    monkeypatch.setattr(simulate.subprocess, "run", lambda *args, **kwargs: None)
    monkeypatch.setattr(compare_module, "load_cases", lambda path, group: [TAPE, PRICES])
    simulation.run()
    document = json.loads((tmp_path / "simulation-small.json").read_text())
    assert document.get("profile") == "phase1-meaning-v1"
    assert document.get("rules_sha256") == PHASE1_RULES_SHA256
    row = document["groups"]["prices"]
    assert row["outcome"] == "inconclusive"
    assert row.get("cases_disagreed") == ["R12-prices"]
    assert row.get("paths")
