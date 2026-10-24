"""Source contract of the read lookup index migration."""

from __future__ import annotations

import importlib.util
import json
from pathlib import Path

from alembic.config import Config
from alembic.script import ScriptDirectory

from app.models.port import Vessel
from app.models.producer import ProducerProject

ROOT = Path(__file__).resolve().parents[2]
PARENT = "prc_20261018_cache"
REVISION = "p5_20261020_read_indexes"
MIGRATION = ROOT / "alembic/versions/p5_20261020_read_indexes.py"
EXPECTED = {
    "ix_p5_vessels_org_id": ("vessels", ("organization_id", "id")),
    "ix_p5_producers_cod_name_id": ("producer_projects", ("cod_year", "name", "id")),
    "ix_p5_producers_status_cod_id": ("producer_projects", ("status", "cod_year", "id")),
}


class RecordingOperations:
    """Stands in for `alembic.op`; an operation it does not define fails the test."""

    def __init__(self):
        self.calls = []

    def execute(self, statement):
        self.calls.append(("execute", str(statement)))

    def create_index(self, name, table, columns, *, unique=False):
        self.calls.append(("create_index", name, table, tuple(columns), unique))

    def drop_index(self, name, *, table_name):
        self.calls.append(("drop_index", name, table_name))


def _migration(operations):
    assert MIGRATION.is_file(), "missing read lookup index migration"
    spec = importlib.util.spec_from_file_location(REVISION, MIGRATION)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.op = operations
    return module


def test_phase5_index_schema_cases():
    script = ScriptDirectory.from_config(Config(str(ROOT / "alembic.ini")))
    assert tuple(script.get_heads()) == (REVISION,)
    assert script.get_revision(REVISION).down_revision == PARENT

    # The reviewed inventory names the same indexes and key order.
    policy = json.loads((ROOT / "tests/rust_contract/phase5/routes.json").read_text())["schema_policy"]
    assert policy["indexes"] == {name: list(columns) for name, (_table, columns) in EXPECTED.items()}

    operations = RecordingOperations()
    _migration(operations).upgrade()
    assert operations.calls == [
        ("execute", 'LOCK TABLE public."vessels", public."producer_projects" IN SHARE MODE NOWAIT'),
        *(("create_index", name, table, columns, False) for name, (table, columns) in EXPECTED.items()),
    ]


def test_phase5_index_downgrade_drops_only_its_indexes():
    operations = RecordingOperations()
    _migration(operations).downgrade()
    assert operations.calls == [
        ("drop_index", name, table) for name, (table, _columns) in reversed(EXPECTED.items())
    ]


def test_phase5_index_models_match_the_migration():
    # Alembic compares indexes, so the models must declare exactly these.
    declared = {
        index.name: (index.table.name, tuple(column.name for column in index.columns), index.unique)
        for model in (Vessel, ProducerProject)
        for index in model.__table__.indexes
        if index.name.startswith("ix_p5_")
    }
    assert declared == {name: (table, columns, False) for name, (table, columns) in EXPECTED.items()}
