"""Replay MCP lane evidence through the authoritative exact-byte comparator."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from scripts.rust_contract.compare import (
    ContractError,
    load_cases,
    recording_differences,
)
from scripts.rust_contract.record import record, stable
from scripts.rust_contract.run_disposable import ROOT


def compare_evidence(entries, evidence: Path, *, gate: bool):
    expected = []
    for entry in entries:
        directory = evidence / (entry["id"] + ("" if gate else "--kyc-off"))
        metadata = json.loads((directory / "meta.json").read_text())
        if metadata["group"] != "mcp" or metadata["KYC_EXECUTION_GATE_ENABLED"] is not gate:
            raise ContractError("MCP evidence state mismatch")
        request = json.loads((directory / "request.json").read_text())
        if request != entry["request"]:
            raise ContractError(f"MCP evidence request mismatch: {entry['id']}")
        expected.append({"id":entry["id"],"request":request,"exchange":json.loads((directory / "response.json").read_text())})
    return recording_differences(expected, stable(entries))


def main():
    import os
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--kyc-gate", choices=["enabled","disabled"], required=True)
    args = parser.parse_args()
    token = os.environ.get("DISPOSABLE_TEST_TOKEN", "")
    if not token:
        raise SystemExit("DISPOSABLE_TEST_TOKEN is required")
    cases = load_cases(ROOT / "tests/rust_contract/cases.json", "mcp")
    entries = record(args.base_url, token, cases)
    problems = compare_evidence(entries,args.evidence,gate=args.kyc_gate=="enabled")
    for problem in problems:
        print(problem)
    print(f"MCP evidence replay: {len(entries)} cases, {len(problems)} differences")
    return int(bool(problems))


if __name__ == "__main__":
    raise SystemExit(main())
