"""Phase 10 Task 34 offline checks for the retained measurement tool.

These tests exercise the final-measurement adapter as data and arithmetic on
the local machine only: no load is generated, no host is contacted and
nothing is measured. The single real run happens much later, under its own
reservation. Every fixture is synthetic and non-sensitive.
"""

import copy
import hashlib
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "tests/rust_contract/phase10"
MEASURE_FINAL = FIXTURES / "measure-final.toml"
WRAPPER = ROOT / "deploy/tests/final-measure.sh"

MIB = 1024 * 1024


def digest_of(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_fixture(name):
    path = FIXTURES / f"{name}.json"
    assert path.is_file(), f"missing offline fixture: {path.name}"
    return json.loads(path.read_text())


def write_seed(tmp_path, document):
    path = tmp_path / "measure-seed.json"
    path.write_text(json.dumps(document, indent=1) + "\n")
    return path


def tmp_config_with_pin(tmp_path, config_path, digest):
    """A config copy in tmp_path whose `seed_sha256` is replaced with the given digest."""
    import re

    text = re.sub(
        r'seed_sha256 = "[0-9a-f]{64}"', f'seed_sha256 = "{digest}"',
        config_path.read_text(),
    )
    copy_path = tmp_path / ("measure-final-" + digest[:8] + ".toml")
    copy_path.write_text(text)
    return copy_path


# ---------------------------------------------------------------------------
# Plan 10.6 named offline fixtures.
# ---------------------------------------------------------------------------


def test_measure_memory_fixtures_carry_the_declared_case():
    fixtures = load_fixture("measure-memory-fixtures")
    declared = next(
        row for row in fixtures["cases"] if row["id"] == "declared-400-100-mib"
    )
    assert declared["python_peak_rss_bytes"] == 400 * MIB
    assert declared["rust_peak_rss_bytes"] == 100 * MIB


def test_measure_seed_matches_the_frozen_workload_shape():
    manifest_digest = "85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215"
    seed = load_fixture("measure-seed")
    assert seed["workload_manifest_sha256"] == manifest_digest
    assert seed["distribution_seed"] == 20261017
    assert seed["corpus_clock"] == "2026-10-15T12:00:00+00:00"
    assert seed["size"] == "small"
    assert set(seed["counts"]) == {"orders", "trades", "news"}
    assert sorted(seed["route_ids"]) == sorted(
        row["id"] for row in seed["workload_rows"]
    )
    assert len(seed["workload_rows"]) == 30


def test_python_memory_baseline_pins_the_private_evidence():
    document = load_fixture("python-memory-baseline")
    identity = document["raw_report"]
    for key in ("path", "sha256", "harness_sha", "harness_version"):
        assert identity[key], f"missing raw-report {key}"
    assert len(identity["sha256"]) == 64 and len(identity["harness_sha"]) == 40
    selection = document["extraction"]
    assert selection["metric"] == "comparison_peak_process_rss_bytes"
    assert selection["scope"]["python_workers"] == 4
    assert selection["scope"]["include_supervisor"] is True
    assert selection["scope"]["exclude"] == [
        "caddy", "postgresql", "load_generator", "test_helper"
    ]
    assert selection["window"]["scrape_interval_seconds"] == 1
    assert selection["window"]["exclude"] == ["warmup", "capacity_ramp"]
    assert document["status"] == "available"
    assert document["evidence_location"].startswith("private-evidence-root:")
    assert len(document["extraction_sha256"]) == 64


# ---------------------------------------------------------------------------
# The twelve Task 34 test names.
# ---------------------------------------------------------------------------


def test_measure_refuses_without_the_pinned_seed(tmp_path):
    from scripts.rust_contract import measure

    with pytest.raises(ValueError, match="seed"):
        measure.load_pinned_seed(None)
    with pytest.raises(ValueError, match="seed"):
        measure.load_pinned_seed(FIXTURES / "absent-seed.json")
    config = MEASURE_FINAL.read_text()
    assert 'seed = "tests/rust_contract/phase10/measure-seed.json"' in config
    seed = load_fixture("measure-seed")
    assert len(seed["seed_sha256"]) == 64
    # The committed phase10 gate-config placeholder makes the standalone gate
    # refuse until the PM records the real digest there; the run config itself
    # carries the exact committed seed digest, and a wrong pin refuses.
    config_document = load_fixture("gate-config-phase10")
    digest = digest_of(FIXTURES / "measure-seed.json")
    assert config_document["measure_seed_sha256"] == "0" * 64, (
        "the committed placeholder refuses until the PM records the real digest"
    )
    assert measure.load_measure_config(MEASURE_FINAL)["size"] == "small"
    wrong = tmp_config_with_pin(tmp_path, MEASURE_FINAL, "0" * 64)
    with pytest.raises(ValueError, match="seed pin"):
        measure.load_measure_config(wrong)


def test_measure_replays_the_exact_schedule_and_route_mix():
    from scripts.rust_contract import measure, workload

    seed = measure.load_pinned_seed(FIXTURES / "measure-seed.json")
    manifest = workload.load()
    replay = measure.replay_schedule(manifest, seed)
    baseline = workload.baseline_reference()
    # Same rows and counts as the frozen baseline, one deterministic order.
    assert replay["order"] == seed["request_order"]
    counts = replay["offered_per_route"]
    expected = {rid: row["offered"] for rid, row in baseline["routes"].items()}
    assert counts == expected
    assert replay["offered_total"] == baseline["offered"] == 4515
    # Replaying the same seed twice is byte-identical (deterministic order).
    again = measure.replay_schedule(manifest, seed)
    assert again == replay


def test_measure_reports_per_route_and_weighted_p95():
    from scripts.rust_contract import measure

    samples = {}
    # Three observations per route: the nearest-rank p95 (ceil(0.95*3)-1 = 2)
    # is the largest value, p * 1.5.
    for route_id, p95 in (("R01", 0.5), ("R02", 0.2), ("R03", 0.1)):
        samples[route_id] = [p95 / 2, p95, p95 * 1.5]
    report = measure.summarize_routes(samples, weights={"R01": 2, "R02": 2, "R03": 4})
    per_route = report["per_route"]
    assert set(per_route) == {"R01", "R02", "R03"}
    assert per_route["R01"]["p95"] == pytest.approx(0.75)
    assert report["representative"]["p95"] == pytest.approx(
        (2 * 0.75 + 2 * 0.3 + 4 * 0.15) / 8
    )
    # The declared mix is derived from the same per-route numbers, never a
    # second run: the frozen baseline satisfies the same identity exactly.
    from scripts.rust_contract import workload

    baseline = workload.baseline_reference()
    weights = workload.representative_weights()
    derived = measure.representative_weight_result(baseline["routes"], weights)
    assert derived["p95"] == pytest.approx(baseline["mix_p95"])


def test_measure_reports_5xx_503_error_rate_observations_and_peak_rss():
    from scripts.rust_contract import measure

    samples = [
        {"route_id": "R01", "seconds": 0.1, "status": 200, "window": "comparison"},
        {"route_id": "R01", "seconds": 0.1, "status": 503, "window": "comparison"},
        {"route_id": "R01", "seconds": 0.1, "status": 500, "window": "comparison"},
        {"route_id": "R01", "seconds": 0.1, "status": 200, "window": "comparison"},
    ]
    counters = [
        {"window": "comparison", "rss": 10 * MIB},
        {"window": "comparison", "rss": 12 * MIB},
        {"window": "comparison", "rss": 11 * MIB},
    ]
    summary = measure.traffic_summary(samples, counters, floor=None)
    assert summary["offered"] == 4
    assert summary["observations"] == 4
    assert summary["errors_5xx"] == 2
    assert summary["busy_503"] == 1
    assert summary["error_rate"] == pytest.approx(0.5)
    assert summary["peak_rss_bytes"] == 12 * MIB
    # Comparison peak counts every scrape inside the window, not one PID.
    assert summary["peak_scope"] == "sum_pid_rss"


def test_measure_extracts_digest_pinned_python_memory_and_computes_absolute_and_percentage_differences():
    from scripts.rust_contract import measure

    fixtures = load_fixture("measure-memory-fixtures")
    declared = next(
        row for row in fixtures["cases"] if row["id"] == "declared-400-100-mib"
    )
    extraction = {
        "status": "available",
        "metric": "comparison_peak_process_rss_bytes",
        "scope": {
            "python_workers": 4,
            "include_supervisor": True,
            "exclude": ["caddy", "postgresql", "load_generator", "test_helper"],
        },
        "window": {"scrape_interval_seconds": 1, "exclude": ["warmup", "capacity_ramp"]},
        "evidence": {"source_sha256": declared["python_raw_report_sha256"],
                     "extraction_sha256": declared["python_extraction_sha256"]},
    }
    rust_peak = declared["rust_peak_rss_bytes"]
    memory = measure.memory_comparison(declared["python_peak_rss_bytes"], rust_peak, extraction)
    assert memory["difference_bytes"] == -300 * MIB
    assert memory["difference_percent"] == pytest.approx(-75.0)
    assert memory["savings_bytes"] == 300 * MIB
    assert memory["savings_percent"] == pytest.approx(75.0)
    assert memory["python_peak_bytes"] == 400 * MIB
    assert memory["rust_peak_bytes"] == 100 * MIB
    # The retained one-second smaps_rollup readings are KiB; conversion is
    # 1024 bytes per KiB before any sum or maximum.
    assert measure.kib_to_bytes(409600) == 400 * MIB


def test_measure_rejects_memory_digest_metric_scope_window_and_worker_mismatch():
    from scripts.rust_contract import measure

    extraction = {
        "status": "available",
        "metric": "comparison_peak_process_rss_bytes",
        "scope": {
            "python_workers": 4,
            "include_supervisor": True,
            "exclude": ["caddy", "postgresql", "load_generator", "test_helper"],
        },
        "window": {"scrape_interval_seconds": 1, "exclude": ["warmup", "capacity_ramp"]},
        "evidence": {"source_sha256": "a" * 64, "extraction_sha256": "b" * 64},
    }
    rust_side = {"metric": extraction["metric"], "scope": extraction["scope"],
                 "window": extraction["window"], "evidence": {"source_sha256": "a" * 64}}
    assert measure.memory_comparison(400 * MIB, 100 * MIB, extraction, rust_side) is not None

    def mutated(field, change):
        broken = copy.deepcopy(extraction)
        change(broken)
        return broken

    cases = {
        "source digest": mutated("e", lambda d: d["evidence"].__setitem__("source_sha256", "0" * 64)),
        "extraction digest": mutated("e", lambda d: d["evidence"].__setitem__("extraction_sha256", "0" * 64)),
        "metric": mutated("m", lambda d: d.__setitem__("metric", "memory.peak")),
        "scope": mutated("s", lambda d: d.__setitem__("scope", {
            "python_workers": 1, "include_supervisor": False,
            "exclude": ["caddy", "postgresql", "load_generator", "test_helper"]})),
        "worker count": mutated("w", lambda d: d["scope"].__setitem__("python_workers", 3)),
        "window": mutated("d", lambda d: d.__setitem__("window", {
            "scrape_interval_seconds": 5, "exclude": ["warmup", "capacity_ramp"]})),
    }
    for name, broken in cases.items():
        with pytest.raises(ValueError) as excinfo:
            measure.memory_comparison(400 * MIB, 100 * MIB, broken, rust_side)
        assert "memory" in str(excinfo.value).lower(), name


def test_measure_missing_comparable_python_memory_reports_unavailable_without_a_run():
    from scripts.rust_contract import measure

    absent = {
        "status": "unavailable",
        "metric": "comparison_peak_process_rss_bytes",
        "reason": "no comparable frozen observation; checked absence",
        "evidence": {"source_sha256": "a" * 64, "extraction_sha256": "c" * 64},
    }
    memory = measure.memory_comparison(None, 100 * MIB, absent)
    assert memory["status"] == "unavailable"
    assert memory["difference_bytes"] is None
    assert memory["difference_percent"] is None
    assert memory["savings_bytes"] is None
    assert memory["savings_percent"] is None
    assert memory["rust_peak_bytes"] == 100 * MIB
    assert "memory savings unavailable" in memory["caveat"]
    # A zero baseline is a missing comparable value, not savings of 100%.
    with pytest.raises(ValueError):
        measure.memory_comparison(0, 100 * MIB, {
            "status": "available", "metric": "comparison_peak_process_rss_bytes",
            "scope": {}, "window": {},
            "evidence": {"source_sha256": "a" * 64, "extraction_sha256": "b" * 64}})


def test_measure_memory_comparison_excludes_warmup_and_ramp():
    from scripts.rust_contract import measure

    readings = [
        {"window": "warmup", "rss": 900 * MIB},
        {"window": "comparison", "rss": 100 * MIB},
        {"window": "comparison", "rss": 120 * MIB},
        {"window": "capacity_ramp", "rss": 500 * MIB},
    ]
    peak = measure.comparison_peak(readings)
    assert peak == 120 * MIB
    # Tagged ramp windows never enter comparison percentiles either.
    samples = [
        {"route_id": "R01", "seconds": 9.0, "status": 200, "window": "capacity_ramp"},
        {"route_id": "R01", "seconds": 0.2, "status": 200, "window": "comparison"},
    ]
    summary = measure.traffic_summary(samples, [], floor=None)
    assert summary["offered"] == 1
    assert summary["ramp"]["offered"] == 1
    assert summary["per_route"]["R01"]["p95"] == pytest.approx(0.2)
    ramp_only = measure.traffic_summary([], [{"window": "capacity_ramp", "rss": 999 * MIB}], floor=None)
    assert ramp_only["peak_rss_bytes"] == 0
    assert ramp_only["ramp"]["peak_rss_bytes"] == 999 * MIB


def test_measure_marks_the_below_floor_caveat():
    from scripts.rust_contract import measure

    baseline = workload_reference()
    # The real schedule offers ~150 per route, far below the 1,000 floor.
    summary = measure.traffic_summary(
        _comparison_samples(per_route=150, status=200), [], floor=1000
    )
    caveat = measure.sample_floor_caveat(summary, floor=1000, needed_per_route=None)
    assert caveat["below_floor"] is True
    assert caveat["message"] and "1,000" in caveat["message"]
    # A per-minute-limited route uses its declared floor instead.
    limited = measure.sample_floor_caveat(summary, floor=1000, needed_per_route={"R01": 150})
    assert limited["below_floor"] is False
    assert limited["message"] == ""
    # A route short of even its reduced floor stays inconclusive, never a pass.
    short = measure.sample_floor_caveat(summary, floor=1000, needed_per_route={"R01": 151})
    assert short["below_floor"] is True
    assert baseline is not None


def test_measure_never_changes_the_service_or_deploy_state():
    from scripts.rust_contract import measure

    # The adapter's write surface is one report; every other path is refused.
    allowed = {measure.REPORT_WRITE_MODE}
    assert allowed == {"report-only"}
    for mode in ("seed-database", "migrate", "restart", "deploy", "cutover"):
        with pytest.raises(ValueError):
            measure.write_mode(mode)
    # The wrapper refuses to run without an offline check or reservation and
    # never names a state-changing flag.
    text = WRAPPER.read_text()
    assert "--offline-check" in text
    assert "--reservation" in text
    for forbidden in ("systemctl", "alembic", "deploy.sh", "install_systemd_units"):
        assert forbidden not in text, forbidden


def test_measure_artifact_is_report_only():
    from scripts.rust_contract import measure

    report = measure.write_report(
        Path("/unused"), {"memory": {"difference_percent": -75.0}}
    )
    assert report["artifact_kind"] == "report"
    assert report["gate"] == "report-only"
    assert report["mutates_service_state"] is False
    document = MEASURE_FINAL.read_text()
    assert "report_only = true" in document
    # No verdict field: the report never asserts a performance pass.
    assert "pass" not in report and "no-go" not in report


def test_measurement_runs_after_cutover_only():
    from scripts.rust_contract import measure

    config = measure.load_measure_config(MEASURE_FINAL)
    assert config["requires_cutover_complete"] is True
    assert config["cutover_evidence"] == "signed post-deletion host inventory (Task 33)"
    # Without the durable marker the tool refuses before any host contact.
    with pytest.raises(ValueError, match="cutover"):
        measure.require_cutover(None)
    with pytest.raises(ValueError, match="cutover"):
        measure.require_cutover({"phase": "pre-removal"})
    marker = {"phase": "post-removal", "signed_inventory_sha256": "a" * 64}
    assert measure.require_cutover(marker) is None


# ---------------------------------------------------------------------------
# Shared helpers.
# ---------------------------------------------------------------------------


def workload_reference():
    from scripts.rust_contract import workload

    return workload.baseline_reference()


def _comparison_samples(per_route, status):
    return [
        {"route_id": "R01", "seconds": 0.1 + n / 10_000, "status": status,
         "window": "comparison"}
        for n in range(per_route)
    ]
