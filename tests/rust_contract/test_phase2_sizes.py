"""Phase 2 size profiles and the step-10 route edge cases.

Task 24 keeps the simulation as correctness evidence (decision 110 removed the
measurement workload).  The declared sizes therefore scale the histories that
the routes page over, while every positive fixture keeps the live caps.
"""
import importlib
from pathlib import Path
from types import SimpleNamespace

import pytest

ROOT = Path(__file__).resolve().parents[2]
MODULE = "scripts.rust_contract"


def sizes_module():
    path = ROOT / "scripts/rust_contract/phase2_sizes.py"
    assert path.is_file(), "missing Phase 2 size profiles"
    return importlib.import_module(f"{MODULE}.phase2_sizes")


def registry():
    from scripts.rust_contract.phase2_scenarios import REGISTRY

    return REGISTRY


def scenario(name):
    assert name in registry(), f"missing registered scenario {name}"
    return registry()[name]


def test_both_declared_sizes_exist_and_scale_every_history():
    module = sizes_module()
    small, large = module.profile("small"), module.profile("large")
    assert (small.name, large.name) == ("small", "large")
    for field in module.SCALED_FIELDS:
        assert getattr(large, field) > getattr(small, field) > 0, field


def test_alert_fixture_stays_inside_the_free_tier_cap():
    module = sizes_module()
    for name in ("small", "large"):
        profile = module.profile(name)
        assert 0 < profile.active_alerts < 5, "a positive fixture must not exceed the live cap"


def test_page_sizes_reach_a_full_page_and_a_second_page():
    module = sizes_module()
    small = module.profile("small")
    assert small.notifications >= 101, "the small history must cross a 100-row page"
    assert small.watchlist_events >= 51, "the small history must cross two 25-row pages"
    large = module.profile("large")
    assert large.notifications >= 1001
    assert large.watchlist_events >= 251
    assert large.feedback_history > 200, "the large feedback history crosses the admin page limit"


def test_unknown_size_is_refused():
    module = sizes_module()
    with pytest.raises(module.SizeError):
        module.profile("xl")


def test_histories_scale_with_the_selected_profile():
    module = sizes_module()
    counts = module.histories(module.profile("small"))
    larger = module.histories(module.profile("large"))
    for table in counts:
        assert larger[table] > counts[table] > 0, table


def test_notifications_scenario_declares_its_history_and_page_actions():
    item = scenario("notification_scope_and_email_state")
    assert item.volume is not None, "the notification history must follow --phase2-size"
    actions = item.actions(SimpleNamespace(size=sizes_module().profile("large")))
    names = {action.name for action in actions}
    assert {"list-empty", "page-full", "page-second", "page-beyond-end"} <= names
    assert {action.route_id for action in actions if action.name.startswith("page-")} == {"N01"}


def test_watchlist_activity_scenario_covers_the_four_unscenaried_routes():
    item = scenario("watchlist-activity")
    assert item.volume is not None
    actions = item.actions(SimpleNamespace(size=sizes_module().profile("large")))
    covered = {action.route_id for action in actions}
    assert {"W01", "W02", "W07", "W08", "W09", "W10"} <= covered
    names = {action.name for action in actions}
    assert {"events-first-page", "events-second-page", "events-last-page",
            "events-invalid-cursor", "event-mark-read", "entry-add", "entry-remove"} <= names


def test_feedback_scenario_declares_the_admin_page_boundary():
    item = scenario("feedback-routes")
    assert item.volume is not None
    actions = list(item.actions(SimpleNamespace(size=sizes_module().profile("large"))))
    queries = [action.path for action in actions if action.method == "GET"]
    assert "/api/admin/feedback?limit=200&offset=0" in queries
    assert "/api/admin/feedback?limit=200&offset=200" in queries


def test_every_new_action_has_a_status_and_a_local_path():
    for name in ("notification_scope_and_email_state", "watchlist-activity", "feedback-routes"):
        item = scenario(name)
        for action in item.actions(SimpleNamespace(size=sizes_module().profile("small"))):
            assert isinstance(action.expected_status, int)
            assert action.path.startswith("/api/")


def test_size_profile_reaches_the_route_context(monkeypatch):
    from scripts.rust_contract import phase2_routes, run_disposable

    seen = {}

    class Connection:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def execute(self, _sql, _params=()):
            return []

    def stop(context):
        seen["size"] = context.size.name
        raise SystemExit

    monkeypatch.setattr(phase2_routes, "connect", lambda *_args, **_kwargs: Connection())
    monkeypatch.setattr(phase2_routes, "snapshot", lambda _connection: {})
    monkeypatch.setattr(run_disposable, "docker", lambda *_args: "")
    runner = SimpleNamespace(start_database=lambda: 49152, sha="a" * 40,
        urls=lambda _port: {}, migrate_and_seed=lambda _env: None,
        container="owned-test", python_bin="python")
    item = SimpleNamespace(name="edge", group="all", setup=lambda _connection: None,
        volume=None, build=(), servers=stop, route_qualification=True)
    with pytest.raises(SystemExit):
        phase2_routes.run_registered(runner, SimpleNamespace(phase2_races=False, phase2_size="large"), [item])
    assert seen["size"] == "large"


def test_volume_hook_runs_once_per_template(monkeypatch):
    from scripts.rust_contract import phase2_routes, run_disposable

    calls = []

    class Connection:
        def __enter__(self):
            return self

        def __exit__(self, *_args):
            pass

        def execute(self, _sql, _params=()):
            return []

    def stop(_context):
        raise SystemExit

    monkeypatch.setattr(phase2_routes, "connect", lambda *_args, **_kwargs: Connection())
    monkeypatch.setattr(phase2_routes, "snapshot", lambda _connection: {})
    monkeypatch.setattr(run_disposable, "docker", lambda *_args: "")
    runner = SimpleNamespace(start_database=lambda: 49152, sha="a" * 40,
        urls=lambda _port: {}, migrate_and_seed=lambda _env: None,
        container="owned-test", python_bin="python")

    def volume(connection, profile):
        calls.append((type(connection).__name__, profile.name))

    item = SimpleNamespace(name="edge", group="all", setup=lambda _connection: None,
        volume=volume, build=(), servers=stop, route_qualification=True)
    with pytest.raises(SystemExit):
        phase2_routes.run_registered(runner, SimpleNamespace(phase2_races=False, phase2_size="small"), [item])
    assert calls == [("Connection", "small")]
