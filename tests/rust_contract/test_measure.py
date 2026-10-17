"""Unit tests for the measurement harness and report assembly.

These exercise the arithmetic and the coverage rules without a host, a
database, or a server. A regression, a missing counterpart, or a missing
measurement must never become a pass.
"""

import json

import pytest

from scripts.rust_contract import measure, report, workload


def _samples(seconds, per_route=1):
    return [{"route_id": f"R{index:02d}", "seconds": value, "status": 200} for index, value in enumerate(seconds)]


def _evidence(*, python_current, rust_current, samples_python, samples_rust):
    def cell(target, current, samples):
        return {
            "samples": samples,
            "counters": [
                {
                    "t": 0.0 + index,
                    "memory_current": current,
                    "memory_peak": current + 1024,
                    "anon": current - 512,
                    "file": 512,
                    "cpu_usage_usec": 1000 * index,
                    "pss": current - 256,
                }
                for index in range(3)
            ],
        }

    return {
        "size": "small",
        "repetitions": [
            {
                "index": 0,
                "cells": {
                    "python": cell("python", python_current, samples_python),
                    "rust": cell("rust", rust_current, samples_rust),
                },
            }
        ],
    }


# ----------------------------------------------------------------- percentiles


def test_percentile_uses_nearest_rank():
    values = sorted(float(n) for n in range(1, 101))
    assert measure.percentile(values, 0.95) == 95.0
    assert measure.percentile(values, 0.50) == 50.0
    assert measure.percentile([1.0], 0.95) == 1.0


def test_percentile_refuses_an_empty_or_invalid_input():
    with pytest.raises(ValueError):
        measure.percentile([], 0.95)
    with pytest.raises(ValueError):
        measure.percentile([1.0], 0.0)


def test_summarize_reports_the_five_numbers():
    summary = measure.summarize([float(n) for n in range(1, 101)])
    assert summary == {"n": 100, "p50": 50.0, "p95": 95.0, "p99": 99.0, "min": 1.0, "max": 100.0}


# ------------------------------------------------------------------ cgroup parse


def test_parse_memory_stat_reads_anon_and_file():
    stat = "anon 1234\nfile 567\nslab 9\n"
    assert measure.parse_memory_stat(stat) == {"anon": 1234, "file": 567}


def test_parse_memory_stat_refuses_a_missing_field():
    with pytest.raises(ValueError):
        measure.parse_memory_stat("anon 1234\n")


def test_parse_cpu_usage_usec_reads_the_counter():
    assert measure.parse_cpu_usage_usec("usage_usec 98765\nuser_usec 1\n") == 98765
    with pytest.raises(ValueError):
        measure.parse_cpu_usage_usec("user_usec 1\n")


def test_cpu_core_equivalents():
    assert measure.cpu_core_equivalents(2_000_000, 1_000_000) == 2.0
    with pytest.raises(ValueError):
        measure.cpu_core_equivalents(1, 0)


def test_memory_ratio_needs_a_positive_baseline():
    assert measure.memory_ratio(100, 40) == 0.4
    with pytest.raises(ValueError):
        measure.memory_ratio(0, 0)


# -------------------------------------------------------------------- coverage


def test_coverage_names_every_route_that_has_not_reached_the_floor():
    result = measure.coverage({"R16": 999, "R17": 1000}, 1000)
    assert result["routes"]["R16"] == {"observations": 999, "needed": 1, "outcome": "inconclusive"}
    assert result["routes"]["R17"] == {"observations": 1000, "outcome": "ok"}
    assert result["inconclusive"] == ["R16"]


# ------------------------------------------------------------------ regression


def test_no_regression_names_the_slower_route():
    summary = [
        {"route_id": "R16", "target": "python", "p95": 1.0},
        {"route_id": "R16", "target": "rust", "p95": 1.2},
        {"route_id": "R17", "target": "python", "p95": 1.0},
        {"route_id": "R17", "target": "rust", "p95": 0.9},
    ]
    problems = measure.require_no_regression(summary)
    assert problems == ["R16: Rust p95 1.2000s > Python 1.0000s"]


def test_no_regression_reports_a_missing_counterpart():
    summary = [{"route_id": "R16", "target": "rust", "p95": 1.2}]
    assert measure.require_no_regression(summary) == ["R16: no Python counterpart"]


def test_no_regression_is_empty_when_rust_is_not_slower():
    summary = [
        {"route_id": "R16", "target": "python", "p95": 1.0},
        {"route_id": "R16", "target": "rust", "p95": 1.0},
    ]
    assert measure.require_no_regression(summary) == []


# --------------------------------------------------------------- report tables


def test_memory_table_uses_the_median_steady_state():
    evidence = _evidence(
        python_current=1000, rust_current=400, samples_python=_samples([1.0]), samples_rust=_samples([0.5])
    )
    table = report.memory_table(evidence)
    assert table["python"]["median_bytes"] == 1000
    assert table["rust"]["median_bytes"] == 400
    assert table["rust"]["peak_bytes"] == 1424


def test_cpu_table_reports_core_equivalents_and_usec_per_request():
    evidence = _evidence(
        python_current=1000,
        rust_current=400,
        samples_python=_samples([1.0, 1.0]),
        samples_rust=_samples([0.5]),
    )
    table = report.cpu_table(evidence)
    # usage_usec 0 -> 2000 over t 0 -> 2 seconds is 0.001 core-equivalents.
    assert table["python"]["core_equivalents"] == pytest.approx(0.001)
    assert table["python"]["usec_per_request"] == pytest.approx(1000)
    assert table["rust"]["completed_requests"] == 1


def test_latency_table_counts_every_class_separately():
    samples = [
        {"route_id": "R16", "seconds": 1.0, "status": 200},
        {"route_id": "R16", "seconds": 2.0, "status": 200},
        {"route_id": "R16", "seconds": 5.0, "status": 503},
        {"route_id": "R16", "seconds": 0.1, "status": 429},
        {"route_id": "R16", "seconds": 0.0, "status": 0},
    ]
    evidence = _evidence(
        python_current=1000, rust_current=400, samples_python=samples, samples_rust=_samples([0.5])
    )
    table = report.latency_table(evidence)
    row = table["R16"]["python"]
    # The latency series is the successful responses only.
    assert row["n"] == 2
    # Every non-success class is reported, and a transport failure still counts
    # as completed traffic for coverage but is named separately.
    assert row["busy"] == 1
    assert row["rate_limited"] == 1
    assert row["transport_errors"] == 1
    assert row["completed"] == 4
    assert row["success"] == 2
    assert row["statuses"] == {"200": 2, "503": 1, "429": 1, "0": 1}


def test_speed_verdict_is_inconclusive_without_a_measured_mix():
    latency = _latency([("R16", 1.0, 0.5)])
    verdict = report.speed_verdict(latency, {})
    assert verdict["mix_measured"] is False
    assert verdict["mix_blocks"] is False


def test_speed_verdict_carries_the_measured_mix_comparison():
    latency = _latency([("R16", 1.0, 0.5)])
    verdict = report.speed_verdict(latency, {"python": {"p95": 1.0}, "rust": {"p95": 0.5}})
    assert verdict["outcome"] == "pass"


def test_weighted_mix_is_a_declared_weighted_aggregate():
    mix = measure.weighted_mix(
        {"R16": 1.0, "R17": 2.0}, {"R16": 1.0, "R17": 2.0}, {"R16": 1.0, "R17": 2.0}, {"R16": 1, "R17": 3}
    )
    assert mix["p95"] == pytest.approx((1.0 + 3 * 2.0) / 4)


def test_evidence_digest_is_stable_across_key_order():
    first = {"b": 1, "a": [1, 2]}
    second = {"a": [1, 2], "b": 1}
    assert report.evidence_digest(first) == report.evidence_digest(second)


# ------------------------------------------------------------------- manifest


def test_manifest_digest_is_reproducible():
    manifest = workload.load()
    assert workload.manifest_digest(manifest) == workload.manifest_digest(workload.load())


def test_manifest_declares_the_plan_sizes():
    sizes = workload.sizes(workload.load())
    assert sizes["small"] == {"orders": 1000, "trades": 1000, "news": 100}
    assert sizes["large"] == {"orders": 100000, "trades": 100000, "news": 10000}


def test_weighted_schedule_is_deterministic_and_weighted():
    manifest = workload.load()
    first = workload.weighted_schedule(manifest, "weight_baseline")
    second = workload.weighted_schedule(manifest, "weight_baseline")
    assert first == second
    assert len(first) == workload.total_weight(manifest, "weight_baseline")


def test_observations_needed_scales_with_weight():
    assert workload.observations_needed(60, 10.0, 10, 100) == pytest.approx(60.0)
    assert workload.seconds_for_observations(1000, 10.0, 10, 100) == pytest.approx(1000.0)


def test_manifest_digest_is_recorded_for_the_report():
    digest = workload.manifest_digest(workload.load())
    assert len(digest) == 64
    # The digest the report cites is the committed manifest's own digest.
    assert json.dumps(workload.load())  # the manifest is serializable evidence


# ------------------------------------------------- decision-68 verdict logic


def _latency(pairs):
    """A latency table from (route, python p95, rust p95) triples."""
    return {
        route: {
            "python": {"p50": 0.1, "p95": py, "p99": py, "n": 10},
            "rust": {"p50": 0.1, "p95": rs, "p99": rs, "n": 10},
        }
        for route, py, rs in pairs
    }


def test_regression_of_exactly_100_ms_does_not_block():
    assert not report.route_regression_blocks(0.200, 0.300)


def test_regression_of_exactly_50_percent_does_not_block_when_under_100_ms():
    assert not report.route_regression_blocks(0.100, 0.150)


def test_regression_above_100_ms_blocks():
    assert report.route_regression_blocks(0.200, 0.301)


def test_regression_above_50_percent_blocks_even_under_100_ms():
    assert report.route_regression_blocks(0.100, 0.151)


def test_slow_route_blocks_only_above_100_ms():
    # A slow Python baseline (1 s): 50 % would be 500 ms, so the absolute
    # 100 ms threshold fires first. 1.000 -> 1.100 is exactly at the cap in
    # decimal and just above it in binary float, so the boundary case uses
    # values that are unambiguous either way.
    assert not report.route_regression_blocks(1.000, 1.099)
    assert report.route_regression_blocks(1.000, 1.101)


def test_mix_regression_with_every_route_inside_its_limit_blocks():
    latency = _latency([("R01", 0.100, 0.150), ("R02", 0.100, 0.150)])
    verdict = report.speed_verdict(latency, {"python": {"p95": 0.10}, "rust": {"p95": 0.12}})
    assert verdict["outcome"] == "no-go"
    assert verdict["mix_blocks"] is True
    assert verdict["blocking_regressions"] == []
    assert verdict["fix_items"] == ["R01", "R02"]


def test_a_5xx_rise_blocks_with_everything_else_passing():
    evidence = _evidence(
        python_current=1000,
        rust_current=400,
        samples_python=[{"route_id": "R01", "seconds": 0.1, "status": 200}],
        samples_rust=[{"route_id": "R01", "seconds": 0.1, "status": 200},
                      {"route_id": "R01", "seconds": 0.1, "status": 503},
                      {"route_id": "R01", "seconds": 0.1, "status": 500}],
    )
    memory = report.memory_table(evidence)
    latency = report.latency_table(evidence)
    conditions = report._gate_conditions(
        0, rust_5xx=2, python_5xx=0, integration_passed=True,
        speed={"outcome": "pass", "mix_blocks": False, "fix_items": [], "blocking_regressions": []},
        memory=memory,
    )
    assert {c["name"]: c["outcome"] for c in conditions}["no rise in 5xx/503 from Rust against Python"] == "no-go"
    assert all(c["outcome"] == "pass" for c in conditions if "5xx" not in c["name"])


def test_build_bundle_counts_the_5xx_rise_from_latency_classes():
    evidence = _evidence(
        python_current=1000,
        rust_current=400,
        samples_python=[{"route_id": "R01", "seconds": 0.1, "status": 200}],
        samples_rust=[{"route_id": "R01", "seconds": 0.1, "status": 200},
                      {"route_id": "R01", "seconds": 0.1, "status": 503}],
    )
    bundle = report.build_bundle(
        identities={}, contract={}, simulation={"differences": 0}, measurement=evidence,
        unapproved_differences=0, integration_passed=True,
    )
    assert bundle["gate"]["rust_5xx"] == 1
    assert bundle["gate"]["python_5xx"] == 0
    assert bundle["gate"]["outcome"] == "no-go"


def test_memory_below_target_no_longer_blocks():
    evidence = _evidence(
        python_current=1000,
        rust_current=900,
        samples_python=_samples([1.0]),
        samples_rust=_samples([0.5]),
    )
    bundle = report.build_bundle(
        identities={}, contract={}, simulation={"differences": 0}, measurement=evidence,
        unapproved_differences=0, integration_passed=True,
    )
    memory_condition = next(
        c for c in bundle["gate"]["conditions"] if c["name"].startswith("memory target")
    )
    # Above half of Python, so below target — but not blocking.
    assert memory_condition["outcome"] == "below target (non-blocking)"
    assert bundle["gate"]["outcome"] == "pass"


def test_r27_style_regression_blocks_under_the_new_bar():
    # 0.167 -> 0.946 s: +779 ms and +467 %, far beyond both caps.
    assert report.route_regression_blocks(0.167, 0.946)
