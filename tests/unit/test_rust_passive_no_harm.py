"""Passive no-harm decisions use synthetic existing-log rows, never requests."""
import importlib.util
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]


def checker():
    spec = importlib.util.spec_from_file_location("rust_passive_no_harm", ROOT / "scripts/rust_passive_no_harm.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rows(count=50, failures=0, duration=0.1, start=1000):
    return [{"ts": start + i, "status": 500 if i < failures else 200, "duration": duration,
             "request": {"method": "GET", "uri": "/api/news?limit=1"}} for i in range(count)]


@pytest.mark.parametrize("failure", ["ready", "new-5xx", "rate", "latency", "error-class"])
def test_each_no_harm_trigger_rolls_back(failure):
    module = checker()
    before, after = rows(), rows(start=2800)
    ready, old_errors, new_errors = True, {"old"}, {"old"}
    if failure == "ready": ready = False
    if failure == "new-5xx": after[0]["status"] = 502
    if failure == "rate":
        before = rows(100, 1)
        after = rows(100, 2, start=2800)
    if failure == "latency": after = rows(duration=0.201, start=2800)
    if failure == "error-class": new_errors.add("new")
    report = module.evaluate(before + after, "simple", 2800, 1800, ready, old_errors, new_errors)
    assert report["decision"] == "ROLLBACK"
    assert report["reasons"]


def test_equal_thresholds_sparse_traffic_and_exact_route_scope():
    module = checker()
    before, after = rows(200, 2), rows(200, 3, duration=0.2, start=2800)
    # Exactly +0.5 percentage points and exactly 2x do not trigger.
    assert module.evaluate(before + after, "simple", 2800, 1800, True, set(), set())["decision"] == "PASS"
    sparse = rows(49) + rows(49, duration=99, start=2800)
    assert module.evaluate(sparse, "simple", 2800, 1800, True, set(), set())["decision"] == "HOLD"
    other = rows()
    for row in other: row["request"]["uri"] = "/api/orderbook/my"
    assert module.evaluate(other, "orderbook", 2800, 1800, True, set(), set())["decision"] == "HOLD"
    for method, path in [("POST", "/api/prices"), ("GET", "/api/prices/secret"), ("GET", "/api/ports/a/b")]:
        assert not module.matches("prices" if "prices" in path else "simple", method, path)
    assert module.matches("prices", "GET", "/api/prices/reference/export/?x=1")


def test_malformed_selected_rows_fail_closed_and_p95_uses_nearest_rank():
    module = checker()
    assert module.p95([1] * 94 + [2] * 6) == 2
    malformed = rows()
    malformed[0]["duration"] = -1
    with pytest.raises(ValueError):
        module.evaluate(malformed, "simple", 2800, 1800, True, set(), set())
    assert module.error_classes([{"MESSAGE": '{"level":"error","error_type":"DatabaseError"}', "_SYSTEMD_UNIT": "verdaxis-rust.service"}]) == {"verdaxis-rust.service:DatabaseError"}


def test_staging_json_access_shape_is_read_without_volume_override():
    module = checker()
    path = ROOT / "tests/fixtures/caddy/staging-access.jsonl"
    report = module.evaluate(module.json_lines([path]), "simple", 2800, 1800, True, set(), set())
    assert report["requests"] == [1, 1]
    assert report["errors_5xx"] == [0, 0]
    assert report["p95_seconds"] == [0.02, 0.03]
    assert report["decision"] == "HOLD"
    # Checklist D's mechanics-only exception is operator policy, not a weakened
    # production passive decision in this shared reader.


def test_passive_cli_reads_files_and_returns_the_decision_exit(tmp_path):
    import datetime
    import json
    import subprocess
    import sys

    access = tmp_path / "access.jsonl"
    access.write_text("".join(json.dumps(row) + "\n" for row in rows() + rows(start=2800)))
    journal = tmp_path / "journal.jsonl"
    journal.write_text("")
    ready = tmp_path / "ready.json"
    ready.write_text(json.dumps(dict(status="ok", db="ok", environment="production", release_sha="a" * 40)))
    command = [sys.executable, str(ROOT / "scripts/rust_passive_no_harm.py"), "--access-log", str(access), "--before-journal", str(journal), "--after-journal", str(journal), "--cutover", datetime.datetime.fromtimestamp(2800, datetime.timezone.utc).isoformat(), "--group", "simple", "--readiness", str(ready), "--environment", "production", "--release-sha", "a" * 40]
    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["decision"] == "PASS"
    ready.write_text("{}")
    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
    assert result.returncode == 1
    assert json.loads(result.stdout)["decision"] == "ROLLBACK"
