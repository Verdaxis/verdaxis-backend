"""The arithmetic vectors stay reproducible from the frozen Python context."""
import json
from pathlib import Path

from scripts.rust_contract.export_voyage_decimal import vectors


def test_decimal_vectors_match_python_context():
    path = Path(__file__).resolve().parents[2] / "deploy/rust/voyage-decimal-vectors.json"
    assert json.loads(path.read_text()) == vectors()


def test_mcp_recorder_writes_only_lane_evidence():
    from scripts.rust_contract.record_mcp import EVIDENCE_ROOT, ROOT

    assert EVIDENCE_ROOT == ROOT / "work/rust-phase1/mcp-evidence"
