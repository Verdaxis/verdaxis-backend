"""Assemble measured evidence into the Task 14a go/no-go report.

Every number here comes from a measured run. A field that was not measured is
rendered as `not measured`; nothing is prefilled. The report states the exact
gate outcome per condition and keeps inconclusive results visible.

Only aggregate, redacted evidence and input digests reach `docs/rust-phase1/`.
Raw samples stay under `work/rust-phase1/`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
from pathlib import Path
from typing import Any

from scripts.rust_contract import workload
from scripts.rust_contract import measure as measure_mod

ROOT = Path(__file__).resolve().parents[2]
NOT_MEASURED = "not measured"


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def evidence_digest(evidence: Any) -> str:
    """One digest over the canonical spelling of an evidence document."""
    return sha256_bytes(workload.canonical_json(evidence).encode())


def _cells(evidence: dict[str, Any]) -> list[dict[str, Any]]:
    """Flatten every (repetition, target) cell into one list."""
    rows = []
    for repetition in evidence["repetitions"]:
        for target, cell in repetition["cells"].items():
            rows.append({"target": target, "repetition": repetition["index"], **cell})
    return rows


def memory_table(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Steady median, p95, peak, anon/file and PSS per runtime."""
    table: dict[str, dict[str, Any]] = {}
    for row in _cells(evidence):
        counters = [c for c in row["counters"] if "memory_current" in c]
        if not counters:
            table[row["target"]] = {"outcome": "inconclusive", "reason": "no cgroup counter"}
            continue
        current = sorted(c["memory_current"] for c in counters)
        peaks = [c["memory_peak"] for c in counters]
        table[row["target"]] = {
            "samples": len(current),
            "median_bytes": statistics.median(current),
            "p95_bytes": measure_mod.percentile(current, 0.95),
            "peak_bytes": max(peaks),
            "anon_median_bytes": statistics.median([c["anon"] for c in counters]),
            "file_median_bytes": statistics.median([c["file"] for c in counters]),
            "pss_median_bytes": statistics.median([c["pss"] for c in counters]),
        }
    return table


def cpu_table(evidence: dict[str, Any]) -> dict[str, dict[str, Any]]:
    """Core-equivalents and CPU microseconds per completed request."""
    table: dict[str, dict[str, Any]] = {}
    for row in _cells(evidence):
        counters = [c for c in row["counters"] if "cpu_usage_usec" in c]
        completed = [s for s in row["samples"] if 200 <= s["status"] < 300]
        if not counters or not completed:
            table[row["target"]] = {"outcome": "inconclusive", "reason": "no CPU counter or no completed request"}
            continue
        elapsed_usec = (counters[-1]["t"] - counters[0]["t"]) * 1_000_000
        usage_usec = counters[-1]["cpu_usage_usec"] - counters[0]["cpu_usage_usec"]
        table[row["target"]] = {
            "completed_requests": len(completed),
            "core_equivalents": measure_mod.cpu_core_equivalents(int(usage_usec), int(elapsed_usec)),
            "usec_per_request": usage_usec / len(completed),
        }
    return table


def latency_table(evidence: dict[str, Any]) -> dict[str, dict[str, dict[str, Any]]]:
    """Per route and per runtime, the measured quantiles and error counts."""
    table: dict[str, dict[str, dict[str, Any]]] = {}
    for row in _cells(evidence):
        for sample in row["samples"]:
            route = table.setdefault(sample["route_id"], {}).setdefault(
                row["target"], {"latencies": [], "errors": 0, "statuses": {}}
            )
            route["statuses"][str(sample["status"])] = route["statuses"].get(str(sample["status"]), 0) + 1
            if 200 <= sample["status"] < 300:
                route["latencies"].append(sample["seconds"])
            else:
                route["errors"] += 1
    summary: dict[str, dict[str, dict[str, Any]]] = {}
    for route_id, targets in table.items():
        summary[route_id] = {}
        for target, data in targets.items():
            if not data["latencies"]:
                summary[route_id][target] = {"outcome": "inconclusive", "errors": data["errors"]}
                continue
            summary[route_id][target] = {
                **measure_mod.summarize(data["latencies"]),
                "errors": data["errors"],
                "statuses": data["statuses"],
            }
    return summary


def mix_table(manifest: dict[str, Any], latency: dict[str, Any]) -> dict[str, dict[str, float]]:
    """The declared representative-mix aggregate quantiles per runtime."""
    weights = {tool["id"]: tool["weight_representative"] for tool in workload.tools(manifest)}
    for route in workload.routes(manifest):
        weights[route["id"]] = route["weight_representative"]
    mix: dict[str, dict[str, float]] = {}
    for target in ("python", "rust"):
        p50, p95, p99 = {}, {}, {}
        for route_id, by_target in latency.items():
            cell = by_target.get(target)
            if cell and "p50" in cell:
                p50[route_id] = cell["p50"]
                p95[route_id] = cell["p95"]
                p99[route_id] = cell["p99"]
        if p50:
            usable = {rid: weights[rid] for rid in p50}
            mix[target] = measure_mod.weighted_mix(p50, p95, p99, usable)
    return mix


def gate_evidence(
    memory: dict[str, Any], latency: dict[str, Any], unapproved_differences: int
) -> dict[str, Any]:
    """The exact inputs to the Rust go/no-go gate, or an inconclusive marker."""
    python_mem = memory.get("python", {})
    rust_mem = memory.get("rust", {})
    if "median_bytes" not in python_mem or "median_bytes" not in rust_mem:
        return {"outcome": "inconclusive", "reason": "memory was not measured"}
    python_p95 = latency.get("__mix__", {}).get("python", {}).get("p95")
    rust_p95 = latency.get("__mix__", {}).get("rust", {}).get("p95")
    if python_p95 is None or rust_p95 is None:
        return {"outcome": "inconclusive", "reason": "the mix p95 was not measured"}
    return {
        "unapproved_differences": unapproved_differences,
        "python_memory": int(python_mem["median_bytes"]),
        "rust_memory": int(rust_mem["median_bytes"]),
        "python_p95": float(python_p95),
        "rust_p95": float(rust_p95),
    }


def write_report(destination: Path, document: str) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(document)


def _bytes_mib(value: Any) -> str:
    return NOT_MEASURED if value is None else f"{value / (1024 * 1024):.1f} MiB"


def _seconds(value: Any) -> str:
    return NOT_MEASURED if value is None else f"{value:.3f}"


def render_markdown(bundle: dict[str, Any]) -> str:
    """Render the go/no-go report from measured values only.

    Every field is read from `bundle`. A key that is absent or null becomes
    `not measured`; nothing is invented.
    """
    identities = bundle.get("identities", {})
    contract = bundle.get("contract", {})
    simulation = bundle.get("simulation", {})
    memory = bundle.get("memory", {})
    cpu = bundle.get("cpu", {})
    latency = bundle.get("latency", {})
    mix = bundle.get("mix", {})
    gate = bundle.get("gate", {})
    coverage = bundle.get("coverage", {})
    workload_info = bundle.get("workload", {})

    lines: list[str] = []
    lines.append("# Rust Phase 1 go/no-go")
    lines.append("")
    lines.append(f"- Decision requested: {bundle.get('decision', 'continue / pause / stop')}.")
    lines.append(f"- Candidate source SHA: {identities.get('candidate_sha') or NOT_MEASURED}")
    lines.append(f"- Python oracle SHA and corpus digest: {identities.get('oracle_sha') or NOT_MEASURED}"
                 f" / {identities.get('corpus_digest') or NOT_MEASURED}")
    lines.append(f"- Schema revision and policy digest: {identities.get('schema_revision') or NOT_MEASURED}"
                 f" / {identities.get('policy_digest') or NOT_MEASURED}")
    lines.append(f"- Rust toolchain, target, libc minimum, Cargo.lock digest, artifact digest: "
                 f"{identities.get('toolchain') or NOT_MEASURED}")
    lines.append(f"- Host/CPU/cgroup/PostgreSQL/PostGIS configuration: "
                 f"{identities.get('host') or NOT_MEASURED}")
    lines.append("- Scope: 27 data GETs, 3 health GETs, 3 MCP transport pairs; 3 MCP tools.")
    lines.append("")
    lines.append("## Contract and simulation")
    lines.append("")
    lines.append(f"- Contract result: {contract.get('total', NOT_MEASURED)} cases, "
                 f"{contract.get('passed', NOT_MEASURED)} passed, {contract.get('failed', NOT_MEASURED)} failed, "
                 f"{contract.get('skipped', NOT_MEASURED)} skipped; "
                 f"{contract.get('differences', NOT_MEASURED)} unapproved differences.")
    lines.append(f"- Contract evidence digest: {contract.get('evidence_digest') or NOT_MEASURED}")
    lines.append(f"- Seeded differential simulation: {simulation.get('summary', NOT_MEASURED)}")
    lines.append(f"- Simulation evidence digest: {simulation.get('evidence_digest') or NOT_MEASURED}")
    lines.append(f"- Workload manifest digest: {workload_info.get('manifest_digest') or NOT_MEASURED}")
    lines.append(f"- Workload sizes: {workload_info.get('sizes', NOT_MEASURED)}")
    lines.append("")
    lines.append("## Memory")
    lines.append("")
    lines.append("| Runtime | steady median | p95 | peak | anon median | file median | PSS median | ratio |")
    lines.append("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for target in ("python", "rust"):
        row = memory.get(target, {})
        ratio = ""
        if target == "rust" and memory.get("python", {}).get("median_bytes"):
            ratio = f"{row.get('median_bytes', 0) / memory['python']['median_bytes']:.2f}"
        lines.append(
            f"| {target} | {_bytes_mib(row.get('median_bytes'))} | {_bytes_mib(row.get('p95_bytes'))} | "
            f"{_bytes_mib(row.get('peak_bytes'))} | {_bytes_mib(row.get('anon_median_bytes'))} | "
            f"{_bytes_mib(row.get('file_median_bytes'))} | {_bytes_mib(row.get('pss_median_bytes'))} | {ratio} |"
        )
    lines.append("")
    lines.append("## CPU")
    lines.append("")
    lines.append("| Runtime | core-equivalents | usec / completed request |")
    lines.append("| --- | ---: | ---: |")
    for target in ("python", "rust"):
        row = cpu.get(target, {})
        lines.append(
            f"| {target} | {row.get('core_equivalents', NOT_MEASURED)} | "
            f"{row.get('usec_per_request', NOT_MEASURED)} |"
        )
    lines.append("")
    lines.append("## Latency (end-to-end through Caddy)")
    lines.append("")
    lines.append("| Route | Python p50/p95/p99 | Rust p50/p95/p99 | Python n | Rust n | errors py/rs |")
    lines.append("| --- | --- | --- | ---: | ---: | --- |")
    for route_id in sorted(latency):
        py = latency[route_id].get("python", {})
        rs = latency[route_id].get("rust", {})
        lines.append(
            f"| {route_id} | {_seconds(py.get('p50'))}/{_seconds(py.get('p95'))}/{_seconds(py.get('p99'))} | "
            f"{_seconds(rs.get('p50'))}/{_seconds(rs.get('p95'))}/{_seconds(rs.get('p99'))} | "
            f"{py.get('n', NOT_MEASURED)} | {rs.get('n', NOT_MEASURED)} | "
            f"{py.get('errors', NOT_MEASURED)}/{rs.get('errors', NOT_MEASURED)} |"
        )
    lines.append("")
    lines.append(f"- Weighted representative mix: Python "
                 f"{_seconds(mix.get('python', {}).get('p95'))} p95, Rust "
                 f"{_seconds(mix.get('rust', {}).get('p95'))} p95.")
    lines.append("")
    lines.append("## Gate")
    lines.append("")
    lines.append(f"- Outcome: {'PASS' if gate.get('passes') else gate.get('outcome', 'inconclusive').upper()}.")
    for condition in gate.get("conditions", []):
        lines.append(f"  - {condition['name']}: {condition['outcome']} ({condition.get('detail', '')})")
    lines.append(f"- Coverage: {coverage.get('summary', NOT_MEASURED)}")
    for route_id in coverage.get("inconclusive", []):
        lines.append(f"  - {route_id}: inconclusive, {coverage['routes'][route_id].get('needed')} more observations needed")
    lines.append("")
    lines.append("## Approved differences (exactly three)")
    lines.append("")
    lines.append("| ID | What changed | Why approved | Proof |")
    lines.append("| --- | --- | --- | --- |")
    lines.append("| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; "
                 "Python's four local counters can admit up to four times more | PM defines documented "
                 "per-client limits as the contract | Rust exact-boundary concurrent burst/reset tests; "
                 "counts excluded from parity |")
    lines.append("| AD-02 | Rust uses one 8-slot public-read admission gate; Python uses 2 slots in each "
                 "of 4 workers | Same total capacity | Mixed-concurrency test; 503 rates recorded |")
    lines.append("| AD-03 | Rust admits `/api/news` through the same 8-slot gate | Every Rust database use "
                 "holds a permit | Two-direction contention test |")
    lines.append("")
    lines.append("- Production status: not authorized by this report.")
    return "\n".join(lines) + "\n"
