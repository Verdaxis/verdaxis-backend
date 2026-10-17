"""Seeded differential simulation: the whole route corpus and MCP tools, both sizes.

For each contract group the harness records Python, Rust, then Python again on
one disposable database that also carries the workload manifest's bulk rows at
the requested size. The two Python recordings form the sandwich: they must agree
before the Rust recording is allowed to decide a match, so a data change between
the calls is classified inconclusive and never counted as a difference or a
pass. Zero unexplained differences is the gate.

The comparison reuses the one authoritative Python comparator. This module adds
no second comparator.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

from scripts.rust_contract import run_disposable as runner
from scripts.rust_contract import workload

ROOT = runner.ROOT


class Simulation(runner.Runner):
    """Run every group's corpus case list under the Python-Rust-Python sandwich."""

    def __init__(
        self, size: str, *, output_dir: Path, keep: bool = False, groups: list[str] | None = None
    ) -> None:
        super().__init__(
            "all", seed=True, keep=keep, seed_mode="corpus", mixed_caddy=False, rust_target="all"
        )
        self.size = size
        self.output_dir = output_dir
        self.manifest = workload.load()
        self.counts = workload.sizes(self.manifest)[size]
        self.groups = groups or list(runner.GROUPS)
        unknown = [group for group in self.groups if group not in runner.GROUPS]
        if unknown:
            raise runner.RunnerError(f"unknown contract group(s): {', '.join(unknown)}")

    def _seed_workload(self, env: dict[str, str]) -> None:
        subprocess.run(
            [self.python_bin, "-m", "scripts.rust_contract.workload_seed", "--size", self.size],
            cwd=ROOT,
            env={**env, "PYTHONPATH": str(ROOT)},
            check=True,
        )

    def _run_group(self, group: str, cases: list[dict[str, Any]]) -> dict[str, Any]:
        """One group, one fresh workload-seeded environment, one sandwich."""
        from scripts.rust_contract.compare import ContractError, recording_differences
        from scripts.rust_contract.record import record, stable

        env = self.start_environment(group)
        try:
            self._seed_workload(env["base_env"])
            first = record(env["python_url"], self.token, cases)
            middle = record(env["rust_url"], self.token, cases)
            third = record(env["python_url"], self.token, cases)
        except ContractError as error:
            return {"outcome": "error", "error": str(error), "cases": len(cases)}
        finally:
            self.stop_environment()

        if recording_differences(first, third):
            return {
                "outcome": "inconclusive",
                "reason": "python sandwich disagreed",
                "cases": len(cases),
            }
        differences = recording_differences(stable(first), stable(middle))
        return {
            "outcome": "match" if not differences else "difference",
            "cases": len(cases),
            "differences": len(differences),
            "paths": differences[:10],
        }

    def run(self) -> None:
        from scripts.rust_contract.compare import load_cases

        subprocess.run(
            self.build_command(),
            cwd=ROOT,
            env={**os.environ, "RELEASE_SHA": self.sha},
            check=True,
        )
        cases_path = ROOT / "tests/rust_contract/cases.json"
        report: dict[str, Any] = {
            "size": self.size,
            "counts": self.counts,
            "source_sha": self.sha,
            "manifest_digest": workload.manifest_digest(self.manifest),
            "groups": {},
            "total_pairs": 0,
            "differences": 0,
            "inconclusive_groups": [],
        }
        for group in self.groups:
            cases = load_cases(cases_path, group)
            outcome = self._run_group(group, cases)
            report["groups"][group] = outcome
            if outcome["outcome"] == "match":
                report["total_pairs"] += outcome["cases"]
            elif outcome["outcome"] == "difference":
                report["total_pairs"] += outcome["cases"]
                report["differences"] += outcome["differences"]
            else:
                report["inconclusive_groups"].append(group)
        report["gate"] = (
            "pass"
            if report["differences"] == 0 and not report["inconclusive_groups"]
            else "no-go"
        )
        out = self.output_dir / f"simulation-{self.size}.json"
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(json.dumps(report, indent=2) + "\n")
        print(f"SIMULATION_JSON {out}")
        print(json.dumps({k: v for k, v in report.items() if k != "groups"}))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--size", required=True, choices=sorted(workload.DECLARED_SIZES))
    parser.add_argument("--output", type=Path, default=ROOT / "work/rust-phase1")
    parser.add_argument("--keep", action="store_true")
    parser.add_argument(
        "--groups",
        default="",
        help="comma-separated contract groups to run (default: every group)",
    )
    args = parser.parse_args(argv)
    groups = [group for group in args.groups.split(",") if group] or None
    simulation = Simulation(args.size, output_dir=args.output, keep=args.keep, groups=groups)
    try:
        simulation.run()
    except (runner.RunnerError, subprocess.CalledProcessError) as error:
        print(f"simulation failed: {error}", file=sys.stderr)
        return 1
    finally:
        simulation.cleanup()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
