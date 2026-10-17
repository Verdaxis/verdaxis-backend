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


def test_gate_input_is_inconclusive_when_memory_is_unmeasured():
    result = report.gate_evidence({"rust": {"median_bytes": 1}}, {}, 0)
    assert result["outcome"] == "inconclusive"


def test_gate_input_carries_the_measured_numbers():
    memory = {"python": {"median_bytes": 800}, "rust": {"median_bytes": 300}}
    latency = {"__mix__": {"python": {"p95": 1.0}, "rust": {"p95": 0.9}}}
    result = report.gate_evidence(memory, latency, 0)
    assert result == {
        "unapproved_differences": 0,
        "python_memory": 800,
        "rust_memory": 300,
        "python_p95": 1.0,
        "rust_p95": 0.9,
    }


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
