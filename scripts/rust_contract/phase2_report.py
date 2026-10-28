"""Validate complete Phase 2 evidence before publishing a scoped JSON verdict.

This module does not run a test, sign an artifact or authorize a deployment.
Required cells come from the committed manifest, never from observed files.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import xml.etree.ElementTree as ET
from datetime import datetime
from pathlib import Path

from scripts.rust_contract.compare import ContractError, _object_without_duplicates
from scripts.rust_contract.measure import percentile, weighted_mix
from scripts.rust_contract.report import (
    route_regression_blocks,
    sha256_file,
    write_report,
)

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "tests/rust_contract/phase2/evidence-manifest.json"


class InvalidEvidence(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise InvalidEvidence(message)


def count(value):
    require(type(value) is int and value >= 0, "invalid nonnegative integer")
    return value


def finite(value, *, positive=False):
    require(type(value) in (int, float) and math.isfinite(value) and (value > 0 if positive else value >= 0), "invalid finite measurement")
    return value


def load(path):
    def invalid(_):
        raise InvalidEvidence("nonfinite JSON")
    try:
        return json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=_object_without_duplicates, parse_constant=invalid)
    except (OSError, UnicodeError, ValueError, ContractError) as error:
        raise InvalidEvidence("missing or malformed evidence") from error


def exact_ids(values, expected):
    require(isinstance(values, list) and all(isinstance(v, str) for v in values), "invalid case IDs")
    require(len(values) == len(set(values)) and set(values) == set(expected), "missing, duplicate or unknown case IDs")


def verified_path(directory, reference):
    require(isinstance(reference, dict) and set(reference) == {"path", "sha256"}, "invalid file reference")
    require(isinstance(reference["path"], str) and isinstance(reference["sha256"], str), "invalid file identity")
    path = (directory / reference["path"]).resolve()
    require(path.is_relative_to(directory.resolve()), "evidence path escapes directory")
    try:
        require(sha256_file(path) == reference["sha256"], "evidence digest mismatch")
    except OSError as error:
        raise InvalidEvidence("missing evidence file") from error
    return path


def validate_identity(identity, manifest, candidate_sha, binary_sha256):
    require(isinstance(identity, dict), "missing identity")
    require(set(manifest["required_identity_fields"]) <= set(identity), "incomplete identity")
    require(re.fullmatch(r"[0-9a-f]{40}", identity["candidate_sha"]) is not None, "invalid candidate SHA")
    require(identity["candidate_sha"] == candidate_sha and identity["release_binary_sha256"] == binary_sha256, "stale candidate bytes")
    require(identity["oracle_sha"] == manifest["oracle_sha"], "oracle SHA mismatch")
    for key in manifest["required_identity_fields"]:
        require(isinstance(identity[key], str) and bool(identity[key]), "empty identity")
        if key.endswith("sha256"):
            require(re.fullmatch(r"[0-9a-f]{64}", identity[key]) is not None, "invalid digest")
    require(identity["normalization_sha256"] == sha256_file(MANIFEST.parent / "normalization.json"), "normalization mismatch")
    require(identity["comparator_profile_sha256"] == sha256_file(ROOT / "scripts/rust_contract/phase2_meaning.py"), "comparator mismatch")
    require(identity["python_database_identity"] != identity["rust_database_identity"], "shared write database")


def validate_run(run, kind, identity, manifest):
    require(isinstance(run, dict) and run.get("kind") == kind and run.get("identity") == identity, "run identity mismatch")
    require(set(manifest["required_run_fields"]) <= set(run), "incomplete run metadata")
    require(run["id"] == kind, "run ID mismatch")
    try:
        start, end = (datetime.fromisoformat(run[key].replace("Z", "+00:00")) for key in ("start", "end"))
        require(start.tzinfo is not None and end.tzinfo is not None and end >= start, "invalid run interval")
    except (TypeError, ValueError, AttributeError) as error:
        raise InvalidEvidence("invalid run interval") from error
    require(count(run["exit_code"]) == 0, "nonzero child exit")
    require(count(run["skips"]) == 0, "skipped required cases")
    count(run["failures"])
    require(re.fullmatch(r"[0-9a-f]{64}", run["input_sha256"]) is not None, "invalid input digest")
    expected = [row["id"] for row in manifest["required_scenarios"]]
    exact_ids(run["expected_cases"], expected)
    exact_ids(run["observed_cases"], expected)
    cells = run.get("cells")
    require(isinstance(cells, list), "missing cells")
    wanted = {(runtime, size, kyc) for runtime in ("python", "rust") for size in ("small", "large") for kyc in (False, True)}
    seen, failures = set(), []
    for cell in cells:
        require(isinstance(cell, dict) and type(cell.get("kyc")) is bool, "invalid policy state")
        key = (cell.get("runtime"), cell.get("size"), cell["kyc"])
        require(key in wanted and key not in seen, "duplicate or unknown runtime cell")
        seen.add(key)
        cases = cell.get("cases")
        require(isinstance(cases, list), "missing case observations")
        exact_ids([case["id"] for case in cases], expected)
        for case in cases:
            require(isinstance(case.get("differences"), list) and all(isinstance(p, str) for p in case["differences"]), "missing comparison observations")
            observed = case["differences"]
            if kind in {"responses", "auth", "staging-read-coverage"}:
                from scripts.rust_contract.compare import compare_responses
                route_id = next(row["route_id"] for row in manifest["required_scenarios"] if row["id"] == case["id"])
                pair = []
                for runtime in ("python", "rust"):
                    raw = case.get(runtime)
                    require(isinstance(raw, dict) and isinstance(raw.get("body"), str), "missing raw response proof")
                    pair.append({**raw, "body": raw["body"].encode("utf-8")})
                observed = compare_responses(*pair, profile="phase2-meaning-v1", route_id=route_id or "N02",
                    rules_sha256=identity["normalization_sha256"])
                require(bool(observed) == bool(case["differences"]), "response count disagrees with raw proof")
            if kind in {"committed-state", "side-effects", "concurrency", "cleanup", "order-write-benchmark", "release-rehearsal"}:
                proof = case.get("proof")
                required = {"python_before", "rust_before", "python_after", "rust_after", "python_effects", "rust_effects", "transaction", "cleanup_remaining"}
                require(isinstance(proof, dict) and set(proof) == required, "missing committed-state/effect proof")
                require(proof["transaction"] in {"committed", "rolled-back", "ambiguous"}, "missing transaction outcome")
                from scripts.rust_contract.compare import compare
                observed = compare(proof["python_after"], proof["rust_after"]) + compare(proof["python_effects"], proof["rust_effects"])
                require(proof["python_before"] == proof["rust_before"], "unequal initial state")
                if proof["cleanup_remaining"]:
                    observed.append("cleanup:remaining")
                require(bool(observed) == bool(case["differences"]), "state count disagrees with proof")
            if observed:
                failures.append(f"{kind}/{case['id']}")
    require(seen == wanted, "missing runtime/size/policy cell")
    # A free-standing failures count cannot replace observed case differences.
    require((run["failures"] == 0) == (not failures), "failure count inconsistent with observations")
    return failures


def junit_failures(directory, reference):
    path = verified_path(directory, reference)
    try:
        raw = path.read_bytes()
        require(len(raw) <= 10_000_000 and b"<!DOCTYPE" not in raw.upper() and b"<!ENTITY" not in raw.upper(), "unsafe JUnit declarations")
        root = ET.fromstring(raw)
    except ET.ParseError as error:
        raise InvalidEvidence("malformed JUnit") from error
    cases = list(root.iter("testcase"))
    require(bool(cases), "zero integration tests")
    require(not any(list(case.iter("skipped")) for case in cases), "skipped integration test")
    return ["integration-junit:failure"] if any(list(case.iter("failure")) or list(case.iter("error")) for case in cases) else []


def measurement_failures(run, manifest):
    require(run.get("floor") == 1000 and type(run["floor"]) is int, "changed sample floor")
    weights = run.get("weights", {})
    require(set(weights) == set(manifest["route_ids"]), "incomplete route weights")
    for value in weights.values():
        finite(value, positive=True)
    require(isinstance(run.get("workload"), str) and bool(run["workload"]), "missing workload identity")
    require(count(run.get("repetitions")) > 0 and isinstance(run.get("runtime_topology"), dict) and bool(run["runtime_topology"]), "missing measurement topology")
    wanted = {(route, runtime, size) for route in manifest["route_ids"] for runtime in ("python", "rust") for size in ("small", "large")}
    rows = {}
    for row in run.get("measurements", []):
        key = (row.get("route_id"), row.get("runtime"), row.get("size"))
        require(key in wanted and key not in rows, "duplicate/unknown measurement cell")
        require(count(row.get("success")) >= 1000, "sample floor not met")
        finite(row.get("p95"), positive=True)
        samples = row.get("samples")
        require(isinstance(samples, list) and len(samples) == row["success"], "missing successful samples")
        for sample in samples:
            finite(sample)
        require(abs(percentile(samples, .95) - row["p95"]) < 1e-12, "p95 not derived from samples")
        statuses = row.get("statuses")
        require(isinstance(statuses, dict) and bool(statuses), "missing status observations")
        for status, value in statuses.items():
            require(re.fullmatch(r"[1-5][0-9]{2}", status) is not None, "invalid HTTP status")
            count(value)
        require(sum(v for s, v in statuses.items() if 200 <= int(s) < 300) == row["success"], "status/sample mismatch")
        count(row.get("transport_failures"))
        rows[key] = row
    require(set(rows) == wanted, "missing runtime/route/size measurement")
    failures, summaries = [], {}
    for size in ("small", "large"):
        quantiles = {runtime: {route: rows[(route, runtime, size)]["p95"] for route in weights} for runtime in ("python", "rust")}
        mix = {runtime: weighted_mix(values, values, values, weights)["p95"] for runtime, values in quantiles.items()}
        pooled = {runtime: percentile([sample for route in weights for sample in rows[(route, runtime, size)]["samples"]], .95) for runtime in ("python", "rust")}
        if mix["rust"] > mix["python"]:
            failures.append(f"measurement/{size}:weighted-p95")
        for route in weights:
            if route_regression_blocks(quantiles["python"][route], quantiles["rust"][route]):
                failures.append(f"measurement/{size}/{route}:route-p95")
            pair = [rows[(route, runtime, size)] for runtime in ("python", "rust")]
            offered = [sum(row["statuses"].values()) + row["transport_failures"] for row in pair]
            require(offered[0] == offered[1], "different offered workload")
            errors = [sum(v for s, v in row["statuses"].items() if int(s) >= 500) for row in pair]
            if errors[1] > errors[0]:
                failures.append(f"measurement/{size}/{route}:5xx-rise")
        summaries[size] = {"weighted_p95": mix, "pooled_p95": pooled}
    return failures, summaries


def evaluate(manifest_path, scope, evidence_dir, *, candidate_sha=None, binary_sha256=None, provenance_key=None):
    result = {"scope": scope, "candidate_sha": candidate_sha, "binary_sha256": binary_sha256,
              "verdict": "INCONCLUSIVE", "failures": [], "incomplete": [], "inputs": {}}
    try:
        manifest = load(manifest_path)
        require(manifest == load(MANIFEST), "unreviewed manifest changes")
        require(scope in manifest["scopes"], "missing or unknown gate scope")
        index = load(evidence_dir / "index.json")
        identity = index.get("identity")
        validate_identity(identity, manifest, candidate_sha, binary_sha256)
        require(index.get("scope") == scope, "candidate scope cannot substitute for cutover")
        inputs = index.get("inputs")
        require(isinstance(inputs, dict), "missing evidence references")
    except (InvalidEvidence, KeyError, TypeError) as error:
        result["incomplete"].append(str(error))
        return result
    for kind in manifest["scopes"][scope]:
        try:
            path = verified_path(evidence_dir, inputs.get(kind))
            run = load(path)
            failures = validate_run(run, kind, identity, manifest)
            if kind == "integration-junit":
                failures += junit_failures(evidence_dir, run.get("junit"))
            if kind == "artifact-provenance":
                from scripts.verify_rust_artifact import (
                    ArtifactError,
                    verify_provenance,
                )
                artifact = run.get("artifact", {})
                require(provenance_key is not None, "missing trusted provenance key")
                record = verified_path(evidence_dir, artifact.get("record"))
                signature = verified_path(evidence_dir, artifact.get("signature"))
                try:
                    verify_provenance(record.read_bytes(), signature.read_bytes(), provenance_key,
                        expected={"source_sha": candidate_sha, "binary_sha256": binary_sha256,
                                  "repository": "Verdaxis/verdaxis-backend", "workflow_path": ".github/workflows/rust-read-ci.yml"})
                except (ArtifactError, OSError) as error:
                    raise InvalidEvidence("unverified artifact provenance") from error
            if kind == "measurement":
                measured_failures, summary = measurement_failures(run, manifest)
                failures += measured_failures
                result["measurement"] = summary
            result["failures"] += failures
            result["inputs"][kind] = inputs[kind]
        except (InvalidEvidence, KeyError, TypeError, ValueError) as error:
            result["incomplete"].append(f"{kind}: {error}")
    result["verdict"] = "NO_GO" if result["failures"] else "INCONCLUSIVE" if result["incomplete"] else "GO"
    return result


def render_markdown(verdict):
    return (f"# Phase 2 evidence verdict\n\n**{verdict['verdict']}**\n\nScope: `{verdict['scope']}`\n\n"
            + "\n".join(f"- Failure: {item}" for item in verdict["failures"])
            + "\n" + "\n".join(f"- Incomplete: {item}" for item in verdict["incomplete"]) + "\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=MANIFEST)
    parser.add_argument("--scope")
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--candidate-sha")
    parser.add_argument("--binary-sha256")
    parser.add_argument("--provenance-key", type=Path)
    parser.add_argument("--out-json", type=Path, required=True)
    parser.add_argument("--out-markdown", type=Path, required=True)
    args = parser.parse_args(argv)
    verdict = evaluate(args.manifest, args.scope, args.evidence_dir, candidate_sha=args.candidate_sha, binary_sha256=args.binary_sha256, provenance_key=args.provenance_key)
    # Publish JSON first. Markdown is rendered only by reading that document.
    write_report(args.out_json, json.dumps(verdict, indent=2, allow_nan=False) + "\n")
    write_report(args.out_markdown, render_markdown(load(args.out_json)))
    return {"GO": 0, "NO_GO": 1, "INCONCLUSIVE": 2}[verdict["verdict"]]


if __name__ == "__main__":
    raise SystemExit(main())
