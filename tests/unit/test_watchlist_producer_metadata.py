"""Producer metadata keeps the existing Python entity projections unchanged."""
import ast
from pathlib import Path

import pytest
from sqlalchemy import inspect, select
from sqlalchemy.dialects import postgresql

from app.model_base import Base
from app.models.market_event import MarketEventOutbox
from app.models.watchlist import WatchlistEvent

ROOT = Path(__file__).resolve().parents[2]


@pytest.mark.parametrize("entity,columns", [
    (WatchlistEvent, ["id", "watchlist_id", "watchlist_target_id", "event_type", "event_payload", "is_read", "created_at"]),
    (MarketEventOutbox, ["id", "event_type", "aggregate_type", "aggregate_id", "participant_org_ids", "payload", "created_at", "dispatched_at", "stream_seq", "delivery_attempts", "last_error"]),
])
def test_default_entity_select_retains_its_literal_column_list(entity, columns):
    compiled = select(entity).compile(dialect=postgresql.dialect())
    assert [column.keyname for column in compiled._result_columns] == columns


@pytest.mark.parametrize("entity,columns", [
    (WatchlistEvent, ["producer", "source_event_id", "source_stream_seq", "scope_attested_at", "derived_event_key"]),
    (MarketEventOutbox, ["producer_evidence"]),
])
def test_producer_columns_are_registered_but_excluded_from_mapping(entity, columns):
    mapper = inspect(entity)
    assert set(columns) <= set(entity.__table__.columns.keys()), "producer metadata is absent"
    assert not set(columns) & set(mapper.attrs.keys())


def test_watermark_metadata_has_no_relationships_or_application_importers():
    assert "watchlist_producer_state" in Base.metadata.tables, "watermark metadata is absent"
    table = Base.metadata.tables["watchlist_producer_state"]
    assert list(table.columns.keys()) == ["singleton", "consumed_through_seq", "updated_at"]
    model = next(mapper for mapper in Base.registry.mappers if mapper.local_table is table)
    assert not list(model.relationships)
    for path in (ROOT / "app").rglob("*.py"):
        if "models" in path.relative_to(ROOT / "app").parts:
            continue
        tree = ast.parse(path.read_text())
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom):
                assert node.module != "app.models.watchlist_producer_state", path
                assert all(alias.name != "WatchlistProducerState" for alias in node.names), path
            elif isinstance(node, ast.Import):
                assert all(alias.name != "app.models.watchlist_producer_state" for alias in node.names), path
