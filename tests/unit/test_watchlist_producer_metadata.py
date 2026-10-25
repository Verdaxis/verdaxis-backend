"""Python projections stay separate from Rust-owned producer schema."""
import ast
from pathlib import Path

import pytest
from sqlalchemy import Column, MetaData, String, Table, inspect, select
from sqlalchemy.dialects import postgresql

from app import migration_drift
from app.models.market_event import MarketEventOutbox
from app.models.watchlist import WatchlistEvent

ROOT = Path(__file__).resolve().parents[2]
RUST_OBJECTS = {
    ("public", "watchlist_producer_state", "table", "watchlist_producer_state"),
    ("public", "watchlist_events", "column", "producer"),
    ("public", "watchlist_events", "column", "source_event_id"),
    ("public", "watchlist_events", "column", "source_stream_seq"),
    ("public", "watchlist_events", "column", "scope_attested_at"),
    ("public", "watchlist_events", "column", "derived_event_key"),
    ("public", "watchlist_events", "check_constraint", "ck_watchlist_events_producer_identity"),
    ("public", "watchlist_events", "index", "uq_watchlist_events_producer_source_target_derived"),
    ("public", "market_event_outbox", "column", "producer_evidence"),
}


@pytest.mark.parametrize("entity,columns", [
    (WatchlistEvent, ["id", "watchlist_id", "watchlist_target_id", "event_type", "event_payload", "is_read", "created_at"]),
    (MarketEventOutbox, ["id", "event_type", "aggregate_type", "aggregate_id", "participant_org_ids", "payload", "created_at", "dispatched_at", "stream_seq", "delivery_attempts", "last_error"]),
])
def test_default_entity_and_table_selects_retain_literal_sql(entity, columns):
    expected = "SELECT " + ", ".join(f"{entity.__tablename__}.{column}" for column in columns) + f" \nFROM {entity.__tablename__}"
    for statement in (select(entity), entity.__table__.select()):
        assert str(statement.compile(dialect=postgresql.dialect())) == expected
    assert list(inspect(entity).columns.keys()) == columns


def test_rust_owned_schema_exclusions_are_exact():
    from tests.postgres.test_phase9_schema import FREEZE_OBJECTS

    assert getattr(migration_drift, "RUST_OWNED_SCHEMA_OBJECTS", set()) == RUST_OBJECTS | FREEZE_OBJECTS


@pytest.mark.parametrize("schema,table,column", [
    ("public", "watchlist_events", "unlisted_extra"),
    ("public", "market_event_outbox", "unlisted_extra"),
    ("other", "market_event_outbox", "producer_evidence"),
    ("public", "other", "producer_evidence"),
])
def test_unlisted_or_other_schema_objects_remain_in_drift(schema, table, column):
    relation = Table(table, MetaData(), Column(column, String()), schema=schema)
    assert migration_drift.include_object(relation.c[column], column, "column", True, None)


def test_application_table_level_reads_use_the_original_outbox_projection():
    callers = []
    for path in (ROOT / "app").rglob("*.py"):
        for node in ast.walk(ast.parse(path.read_text())):
            if isinstance(node, ast.Attribute) and node.attr == "__table__" and isinstance(node.value, ast.Name) and node.value.id in {"WatchlistEvent", "MarketEventOutbox"}:
                callers.append((path.relative_to(ROOT).as_posix(), node.value.id))
    assert sorted(callers) == [("app/services/market_event_dispatch.py", "MarketEventOutbox")] * 3
