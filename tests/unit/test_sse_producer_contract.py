"""Regression checks for append-only SSE provenance producer coverage."""
from pathlib import Path


def test_trade_created_producers_append_trade_provenance():
    root = Path(__file__).resolve().parents[2]
    producer_files = [
        root / "app/routers/trades.py",
        root / "app/routers/rfq.py",
        root / "app/routers/negotiations.py",
    ]

    for path in producer_files:
        text = path.read_text()
        marker = 'event_bus.publish("trades", "trade_created"'
        index = text.find(marker)
        while index != -1:
            snippet = text[index:index + 420]
            assert "trade_activity_provenance(" in snippet, f"{path} trade_created producer lacks provenance"
            index = text.find(marker, index + len(marker))
