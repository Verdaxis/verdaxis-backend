"""Successor recording comparison requires explicit reviewed evidence."""
import pytest

from scripts.rust_contract import compare
from scripts.rust_contract.phase2_meaning import PHASE1_RULES_SHA256


def recording(body):
    return [{"id": "R06-tape", "request": {"method": "GET", "path": "/api/trade-tape"},
             "exchange": compare.capture(200, {"content-type": "application/json"}, body), "raw_body": body}]


def test_successor_requires_reviewed_profile_and_complete_requests():
    entries = recording(b'{"items":[],"total":0,"market_hours":true}')
    assert compare.recording_differences(entries, [], profile="phase1-meaning-v1", rules_sha256=PHASE1_RULES_SHA256)
    with pytest.raises(compare.ContractError):
        compare.recording_differences(entries, entries, profile="phase1-meaning-v1", rules_sha256="")


def test_successor_does_not_fall_back_to_bytes():
    left = recording(b'{"items":[],"total":0,"market_hours":true}')
    right = recording(b'{ "market_hours":true,"total":0.0,"items":[] }')
    assert compare.recording_differences(left, right, profile="phase1-meaning-v1", rules_sha256=PHASE1_RULES_SHA256) == []
    assert compare.recording_differences(left, right)
