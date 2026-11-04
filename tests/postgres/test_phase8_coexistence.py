"""Source facts for the Python writers that keep running beside the Rust market writers.

These checks need no database. The concurrent schedules against a disposable
PostgreSQL database are the Rust target
``rs/read/tests/market_write_pg_residual.rs``, which drives the unchanged
writers through ``tests/postgres/phase8_residual_driver.py``.
"""

from __future__ import annotations

import inspect
import re
from datetime import timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SHARED_TABLES = (
    "orderbook_orders",
    "trades",
    "inventory_items",
    "market_event_outbox",
    "watchlist_events",
)
SHARED_MODELS = ("OrderBookOrder", "Trade", "InventoryItem", "MarketEventOutbox", "WatchlistEvent")
_TABLE_ALTERNATIVES = "|".join(SHARED_TABLES)
_MODEL_ALTERNATIVES = "|".join(SHARED_MODELS)
WRITE_PATTERN = re.compile(
    rf"(INSERT INTO|UPDATE|DELETE FROM|TRUNCATE)\s+({_TABLE_ALTERNATIVES})\b"
    rf"|\b(delete|update|insert)\(({_MODEL_ALTERNATIVES})\)"
    rf"|(?<![\w.])(?<!class )({_MODEL_ALTERNATIVES})\("
    r"|\b(enqueue_market_events|commit_market_events)\("
)

# Route writers whose routes move to Rust in the route switch, with the
# services only they call.
SWITCHED_ROUTE_WRITERS = {
    "app/routers/orderbook.py",
    "app/routers/trades.py",
    "app/routers/inventory.py",
    "app/routers/market_support.py",
    "app/services/order_lifecycle.py",
    "app/services/watchlist_events.py",
}
# Writers that keep running after the switch; the Rust target covers the first
# five, the dispatcher belongs to the stream owner.
RETAINED_WRITERS = {
    "app/services/demo_activity.py",
    "scripts/run_demo_activity.py",
    "app/services/matching_engine.py",  # called by the demo tick for its own pair
    "app/services/market_quarantine.py",
    "app/seeds/market_seed.py",
    "app/services/market_event_dispatch.py",
    # Attested staging or disposable seed scripts (``seed_connection``).
    "scripts/seed_port_inventory.py",
    "scripts/seed_realistic_orderbook.py",
}
# Phase 7 routes: the edge answers 404 for them in the same switch.
PHASE7_ROUTE_WRITERS = {
    "app/routers/negotiations.py",
    "app/routers/rfq.py",
    "app/routers/supplier_offers.py",
}
# Account and KYC routes append organization invalidation events; their route
# owners are earlier phases.
OTHER_ROUTE_OWNERS = {
    "app/routers/auth_simple.py",
    "app/routers/kyc.py",
}
# Definitions and developer tooling, not runtime writers.
DEFINITIONS_AND_TOOLING = {
    "app/services/market_events.py",
    "scripts/phase0/copy.py",
}
TOOLING_PREFIXES = ("scripts/rust_contract/",)

RETAINED_SOURCES_WITHOUT_EMITTER = (
    "app/services/demo_activity.py",
    "scripts/run_demo_activity.py",
    "app/seeds/market_seed.py",
    "app/services/market_quarantine.py",
    "app/services/order_expiry_reminders.py",
)


def _writer_files() -> set[str]:
    found = set()
    for folder in ("app", "scripts"):
        for path in (ROOT / folder).rglob("*.py"):
            relative = path.relative_to(ROOT).as_posix()
            if relative.startswith("app/models/"):
                continue
            if WRITE_PATTERN.search(path.read_text(encoding="utf-8")):
                found.add(relative)
    return found


def test_residual_writer_inventory_is_complete():
    listed = (
        SWITCHED_ROUTE_WRITERS
        | RETAINED_WRITERS
        | PHASE7_ROUTE_WRITERS
        | OTHER_ROUTE_OWNERS
        | DEFINITIONS_AND_TOOLING
    )
    found = _writer_files()
    unlisted = sorted(
        path for path in found - listed if not path.startswith(TOOLING_PREFIXES)
    )
    assert unlisted == [], f"unlisted writers of shared market tables: {unlisted}"
    stale = sorted(listed - found)
    assert stale == [], f"listed files no longer write shared market tables: {stale}"


def test_no_retained_python_writer_emits_watchlist_events_or_producer_evidence():
    for relative in RETAINED_SOURCES_WITHOUT_EMITTER:
        source = (ROOT / relative).read_text(encoding="utf-8")
        assert not re.search(r"INSERT INTO\s+watchlist_events", source), relative
        assert "WatchlistEvent(" not in source, relative
        assert "producer_evidence" not in source, relative
    demo_cli = (ROOT / "scripts/run_demo_activity.py").read_text(encoding="utf-8")
    assert re.findall(r'event_type="([^"]+)"', demo_cli) == ["demo_market_changed"]


def test_reminder_and_slice_lock_constants_are_pinned():
    from app.services import market_locks, order_expiry_reminders

    assert order_expiry_reminders.REMINDER_LOCK_ID == 8246192048
    assert order_expiry_reminders.EMAIL_RETRY_DELAY == timedelta(minutes=15)
    assert order_expiry_reminders.EMAIL_RETRY_WINDOW == timedelta(hours=23)
    assert order_expiry_reminders.REMINDER_LEAD_TIME == timedelta(hours=48)
    assert market_locks.MARKET_LOCK_TIMEOUT == "500ms"
    assert market_locks.MARKET_STATEMENT_TIMEOUT == "5s"

    quarantine = (ROOT / "app/services/market_quarantine.py").read_text(encoding="utf-8")
    assert "SET LOCAL lock_timeout = '500ms'" in quarantine
    assert "SET LOCAL statement_timeout = '5s'" in quarantine

    scan = inspect.getsource(order_expiry_reminders.remind_expiring_orders)
    payload = scan[scan.index("data={") : scan.index("created_at=reference")]
    keys = re.findall(r'"(\w+)":', payload)
    assert keys == [
        "event",
        "order_id",
        "order_key",
        "expiry_epoch",
        "expires_at",
        "_email_only",
        "_email_status",
        "_email_payload",
        "_email_retry_at",
        "_email_deadline",
    ]
