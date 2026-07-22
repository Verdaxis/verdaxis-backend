from __future__ import annotations

import importlib.util
import json
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "deploy/monitor/outbox_backlog_probe.py"


def load_module():
    spec = importlib.util.spec_from_file_location("outbox_backlog_probe", MODULE_PATH)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_parse_backlog_accepts_exactly_one_two_column_row():
    module = load_module()
    assert module.parse_backlog("42|17\n") == (42, 17)
    assert module.parse_backlog("\n 0|0 \n") == (0, 0)


@pytest.mark.parametrize(
    "output",
    ["", "1|2\n3|4\n", "1\n", "a|b\n", "-1|5\n", "5|-1\n", "1|2|3\n"],
)
def test_parse_backlog_rejects_malformed_or_negative_output(output):
    module = load_module()
    with pytest.raises(module.ProbeError):
        module.parse_backlog(output)


def test_evaluate_flags_pending_count_and_age_independently():
    module = load_module()
    ok = module.evaluate(3, 4, max_pending=1000, max_age_seconds=300)
    assert ok["ok"] is True and ok["breaches"] == []

    backlog = module.evaluate(1001, 4, max_pending=1000, max_age_seconds=300)
    assert backlog["ok"] is False and backlog["breaches"] == ["pending_count"]

    # The wedged-but-alive leader signature: tiny backlog, unbounded age.
    wedged = module.evaluate(1, 301, max_pending=1000, max_age_seconds=300)
    assert wedged["ok"] is False and wedged["breaches"] == ["oldest_pending_seconds"]


def test_main_exit_codes_and_json_shape(capsys):
    module = load_module()

    def healthy(dsn, timeout):
        assert dsn == "dbname=verdaxis user=verdaxis_backup"
        assert timeout == 20
        return "0|0\n"

    assert module.main(["--dsn", "dbname=verdaxis user=verdaxis_backup"], query_runner=healthy) == 0
    status = json.loads(capsys.readouterr().out)
    assert status["ok"] is True
    assert status["pending_count"] == 0
    assert status["oldest_pending_seconds"] == 0

    assert (
        module.main(
            ["--dsn", "x", "--max-age-seconds", "300"],
            query_runner=lambda dsn, timeout: "2|9999\n",
        )
        == 1
    )
    breached = json.loads(capsys.readouterr().out)
    assert breached["breaches"] == ["oldest_pending_seconds"]

    def broken(dsn, timeout):
        raise module.ProbeError("psql failed")

    assert module.main(["--dsn", "x"], query_runner=broken) == 2
    error = json.loads(capsys.readouterr().out)
    assert error["ok"] is False and "psql failed" in error["error"]


def test_query_is_read_only_and_targets_unsequenced_rows():
    module = load_module()
    query = module.BACKLOG_QUERY.upper()
    assert query.startswith("SELECT ")
    assert "STREAM_SEQ IS NULL" in query
    for verb in ("INSERT", "UPDATE", "DELETE", "TRUNCATE", "DROP"):
        assert verb not in query


def test_probe_is_not_armed_by_any_unit():
    monitor = ROOT / "deploy/monitor"
    for unit in list(monitor.glob("*.service")) + list(monitor.glob("*.timer")):
        assert "outbox_backlog_probe" not in unit.read_text(), unit.name
