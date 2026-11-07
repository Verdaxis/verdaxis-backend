"""Synthetic test that consumes only the committed data fixture (fixture data only)."""
import json
from pathlib import Path

FIXTURE = Path(__file__).resolve().parents[1] / "fixtures" / "market-data.json"


def test_market_data_fixture_loads():
    payload = json.loads(FIXTURE.read_text())
    assert payload["schema_version"] == 1
