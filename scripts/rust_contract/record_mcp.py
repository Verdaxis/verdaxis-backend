"""Record only Task 11 MCP against a disposable populated Python oracle.

Reuse the shared runner without changing its other groups or golden ownership.
Each KYC state is recorded twice before any bytes are published.
"""
from __future__ import annotations

import hashlib
import json

from scripts.rust_contract.record import differences, record, write_golden
from scripts.rust_contract.run_disposable import (
    CORPUS_CLOCK,
    ORACLE_TIMEZONE,
    ROOT,
    Runner,
    RunnerError,
)

EVIDENCE_ROOT = ROOT / "work/rust-phase1/mcp-evidence"


class McpRecorder(Runner):
    def record_groups(self, python_url: str) -> None:
        cases = json.loads((ROOT / "tests/rust_contract/cases.json").read_text())["groups"]["mcp"]
        first = record(python_url, self.token, cases)
        second = record(python_url, self.token, cases)
        problems = differences(first, second)
        if problems:
            raise RunnerError(f"MCP oracle is unstable: {len(problems)} differences")
        fixtures = (ROOT / "work/rust-phase1/task4b-fixtures.json").read_bytes()
        entries = first if self.gate else [{**entry, "id": entry["id"] + "--kyc-off"} for entry in first]
        write_golden(EVIDENCE_ROOT, self.sha, entries, meta={
            "source_sha": self.sha,
            "schema_revision": "kycd_20261015_staging",
            "timezone": ORACLE_TIMEZONE,
            "clock": CORPUS_CLOCK,
            "group": "mcp",
            "KYC_EXECUTION_GATE_ENABLED": self.gate,
            "seed_mode": "corpus",
            "seed_manifest_sha256": hashlib.sha256(fixtures).hexdigest(),
        })
        schemas = (ROOT / "deploy/rust/mcp-tools.json").read_bytes()
        for entry in entries:
            (EVIDENCE_ROOT / "golden" / self.sha / entry["id"] / "schema.json").write_bytes(schemas)
        print(f"MCP golden: {len(entries)} cases stable across two passes; KYC={self.gate}; SHA={self.sha}", flush=True)


def main():
    for enabled in (True, False):
        runner = McpRecorder("mcp", seed=True, keep=False, seed_mode="corpus", record_all=True, gate=enabled)
        try:
            runner.run()
        finally:
            runner.cleanup()


if __name__ == "__main__":
    main()
