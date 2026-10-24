"""Referral barriers must observe their own scenario's databases."""
import importlib
from types import SimpleNamespace


def test_allocation_race_uses_context_cells(monkeypatch):
    importlib.import_module("scripts.rust_contract.phase2_scenarios")
    module = importlib.import_module("scripts.rust_contract.referral_allocation")
    opened = []

    class Connection:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def execute(self, *_args):
            pass

    def connect(_port, database, **_kwargs):
        opened.append(database)
        return Connection()

    monkeypatch.setattr(module, "connect", connect)
    monkeypatch.setattr(module, "wait_for", lambda *_args: None)
    databases = ("phase2_route_8_python_test", "phase2_route_8_rust_test")
    context = SimpleNamespace(environment={"SEED_DATABASE_URL": "postgresql://127.0.0.1:49152/test"},
                              race_databases=databases)
    assert len(list(module.races(context))) == 4
    assert opened == list(databases) * 4
