"""Publication ledger for Task 10b; recordings remain integration-owned."""
import json
from pathlib import Path


def test_curve_publication_contains_acceptance_regressions():
    path = Path(__file__).with_name("cases.json")
    cases = json.loads(path.read_text())["groups"]["curves_canonical"]
    ids = [case["id"] for case in cases]
    assert len(ids) == len(set(ids))
    assert {
        "R11-uuid-upper", "R11-uuid-compact", "R11-uuid-braces", "R11-uuid-urn",
        "R10b-invalid-board", "R10-depth", "R10-trusted-signals", "R10-bulk",
        "R09-bulk", "R09-subsecond", "R10b-duplicates", "R10b-unknown-query",
    } <= set(ids)
    subsecond = next(case for case in cases if case["id"] == "R09-subsecond")
    assert "windows=2031-05&windows=2031-06" in subsecond["query"]
