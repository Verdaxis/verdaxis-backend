"""Portable checks for the real-runner adapter; these do not claim host acceptance."""

import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

import pytest


PATH = Path(__file__).resolve().parents[2] / "deploy/tests/rehearsal/w2.py"
SPEC = importlib.util.spec_from_file_location("rehearsal_w2", PATH)
w2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(w2)


@pytest.mark.parametrize("environment,count", [("production", 6), ("staging", 4)])
def test_job_timer_inventory_uses_real_environment_scope(environment, count):
    groups = w2.unit_names(environment)
    assert tuple(groups) == w2.GROUPS
    timers = [
        timer
        for group, services in groups.items()
        if group not in ("local-monitor", "external-recovery")
        for timer in w2.timer_names(group, services)
    ]
    assert len(timers) == count
    assert len(timers) == len(set(timers))
    if environment == "staging":
        assert groups["reminders"] == groups["onboarding"] == []


def test_alert_instances_bind_the_actual_template(tmp_path):
    with patch.object(w2, "UNIT_DIRECTORY", tmp_path):
        assert (
            w2.unit_file("verdaxis-monitor-alert@health.service")
            == tmp_path / "verdaxis-monitor-alert@.service"
        )
        source = w2.source_file(
            SimpleNamespace(CHECKOUT=tmp_path), "verdaxis-monitor-alert@backup.service"
        )
        assert source == tmp_path / "deploy/monitor/verdaxis-monitor-alert@.service"


@pytest.mark.parametrize(
    "field,value",
    [
        ("InvocationID", "a" * 32),
        ("Result", "exit-code"),
        ("ExecMainStatus", "3"),
        ("ConditionResult", "no"),
        ("AssertResult", "no"),
        ("ActiveState", "active"),
        ("Job", "17"),
        ("NeedDaemonReload", "yes"),
    ],
)
def test_unit_probe_refuses_reused_failed_skipped_or_busy_run(field, value):
    row = dict(
        InvocationID="b" * 32,
        Result="success",
        ExecMainStatus="0",
        ConditionResult="yes",
        AssertResult="yes",
        ActiveState="inactive",
        Job="0",
        NeedDaemonReload="no",
    )
    row[field] = value
    with patch.object(w2, "observation", return_value=row):
        with pytest.raises(RuntimeError):
            w2.successful_run(None, "verdaxis-health.service", "a" * 32)


def test_persisted_duplicate_probe_refuses_nonzero_count():
    queries = []

    def sql(query):
        queries.append(query)
        if "json_build_object" in query:
            return json.dumps(dict(count=0, rows=""))
        return "1" if "news_items" in query else "0"

    with pytest.raises(RuntimeError, match="duplicate"):
        w2.table_snapshot(SimpleNamespace(sql=sql))
    assert len(queries) == len(w2.EFFECT_TABLES) + 4
    assert any("expiry_epoch" in query for query in queries)
    assert any("buyer_provenance" in query for query in queries)


def test_source_launcher_symlink_is_not_an_ownership_input(tmp_path):
    source = tmp_path / "actual.py"
    source.write_text("print('retained')\n")
    redirected = tmp_path / "redirected.py"
    redirected.symlink_to(source)
    with pytest.raises(RuntimeError, match="regular"):
        w2.file_record(SimpleNamespace(), redirected)


def test_live_external_origin_is_refused_before_kernel_inspection():
    stack = SimpleNamespace(
        run=lambda *args: pytest.fail("kernel inspection must not precede DNS refusal")
    )
    with patch.object(
        w2.socket, "getaddrinfo", return_value=[(2, 1, 6, "", ("203.0.113.10", 443))]
    ):
        with pytest.raises(RuntimeError, match="loopback"):
            w2.network_guard(stack)


def test_network_rule_has_valid_literal_interface_name():
    commands = []

    def run(*args, **kwargs):
        commands.append((args, kwargs))
        return (
            json.dumps(dict(nftables=[]))
            if args[:4] == ("nft", "--json", "list", "tables")
            else ""
        )

    with patch.object(
        w2.pwd,
        "getpwall",
        return_value=[SimpleNamespace(pw_name="verdaxis-prod", pw_uid=991)],
    ):
        assert w2.install_network_guard(SimpleNamespace(run=run)) == [0, 991]
    script = next(
        kwargs["data"].decode()
        for args, kwargs in commands
        if args == ("nft", "-f", "-")
    )
    assert 'oifname != "lo"' in script
    assert '\\"lo\\"' not in script
    assert "meta skuid { 0, 991 } reject" in script
