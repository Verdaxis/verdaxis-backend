"""Client journeys for the six Phase 2 CRUD groups.

Each journey replays the call sequence in `verdaxis-frontend/src/services/api.ts`
and the components that use it, then asserts the fields, types and status codes
those clients read.  A browser run is not part of this evidence.
"""
import importlib

import pytest

GROUPS = ("notifications", "preferences", "feedback", "watchlists", "alerts", "referrals")


def module():
    path = __import__("pathlib").Path(__file__).resolve().parents[2] / "scripts/rust_contract/phase2_journeys.py"
    assert path.is_file(), "missing Phase 2 client journeys"
    return importlib.import_module("scripts.rust_contract.phase2_journeys")


def notification(identifier="30000000-0000-4000-8000-000000000001", **changes):
    value = {
        "id": identifier,
        "type": "SYSTEM",
        "title": "Title",
        "message": "Message",
        "data": {"order_id": "x"},
        "is_read": False,
        "created_at": "2026-10-19T10:00:00Z",
    }
    value.update(changes)
    return value


def alert(identifier="50000000-0000-4000-8000-000000000001", **changes):
    value = {
        "id": identifier,
        "org_id": "10000000-0000-4000-8000-000000000001",
        "product_id": "c4cecebc-3d2d-5840-8021-57a9a11bc673",
        "product_name": "B30",
        "market_product": "B30",
        "delivery_point_id": None,
        "direction": "above",
        "threshold_usd": "812.50",
        "is_active": True,
        "triggered_at": None,
        "created_at": "2026-10-19T10:00:00Z",
    }
    value.update(changes)
    return value


def test_all_six_groups_have_an_ordered_journey():
    journeys = module().journeys()
    assert tuple(journeys) == GROUPS
    for group, calls in journeys.items():
        assert calls, group
        assert [call.group for call in calls] == [group] * len(calls)
        assert [call.status for call in calls]


def test_notifications_journey_matches_the_context_call_sequence():
    calls = module().journeys()["notifications"]
    assert [(call.method, call.path) for call in calls[:4]] == [
        ("GET", "/api/notifications"),
        ("GET", "/api/notifications/unread-count"),
        ("PATCH", "/api/notifications/{notification_id}/read"),
        ("PATCH", "/api/notifications/read-all"),
    ]
    assert calls[2].placeholders == ("notification_id",)


def test_preferences_journey_reads_before_writing_each_namespace():
    calls = module().journeys()["preferences"]
    assert (calls[0].method, calls[0].path) == ("GET", "/api/users/me/preferences")
    writes = [call for call in calls if call.method == "PUT"]
    assert [call.path for call in writes] == ["/api/users/me/preferences/{namespace}"]
    assert calls[-1].method == "GET"


def test_feedback_journey_submits_then_reads_the_admin_page():
    calls = module().journeys()["feedback"]
    assert (calls[0].method, calls[0].path, calls[0].status) == ("POST", "/api/feedback", 201)
    admin = [call for call in calls if call.path.startswith("/api/admin/feedback")]
    assert admin and admin[0].method == "GET"
    assert admin[0].query == {"limit": "100", "offset": "0"}


def test_watchlists_journey_follows_the_radar_hook():
    calls = module().journeys()["watchlists"]
    paths = [(call.method, call.path) for call in calls]
    assert ("POST", "/api/watchlists/me") in paths
    assert ("GET", "/api/watchlists/{watchlist_id}/events") in paths
    assert ("PATCH", "/api/watchlists/{watchlist_id}/events/{event_id}") in paths
    assert ("POST", "/api/watchlists/{watchlist_id}/targets") in paths
    assert ("DELETE", "/api/watchlists/{watchlist_id}/targets/{target_id}") in paths
    assert ("POST", "/api/watchlists") in paths
    assert ("POST", "/api/watchlists/{watchlist_id}/entries") in paths
    assert ("DELETE", "/api/watchlists/{watchlist_id}/entries/{entry_id}") in paths
    assert ("DELETE", "/api/watchlists/{watchlist_id}") in paths
    events = next(call for call in calls if call.path.endswith("/events"))
    assert events.query["limit"] == "25"


def test_alerts_journey_lists_creates_then_deletes():
    calls = module().journeys()["alerts"]
    assert [(call.method, call.status) for call in calls] == [
        ("GET", 200), ("POST", 201), ("GET", 200), ("DELETE", 204), ("GET", 200)
    ]
    assert calls[1].json_body == {
        "product_id": "c4cecebc-3d2d-5840-8021-57a9a11bc673",
        "direction": "above",
        "threshold_usd": 900,
    }


def test_referrals_journey_reads_code_stats_and_leaderboard_then_invites():
    calls = module().journeys()["referrals"]
    assert [(call.method, call.path) for call in calls[:3]] == [
        ("POST", "/api/referrals/my-code"),
        ("GET", "/api/referrals/my-referrals"),
        ("GET", "/api/referrals/leaderboard"),
    ]
    assert any(call.path == "/api/referrals/invite" and call.method == "POST" for call in calls)


def test_notification_projection_requires_frontend_types():
    check = module().check_notification_list
    check([notification()])
    with pytest.raises(module.JourneyError):
        check([notification(is_read="false")])
    with pytest.raises(module.JourneyError):
        check([notification(created_at="not-a-time")])
    with pytest.raises(module.JourneyError):
        check([{"id": "x"}])
    with pytest.raises(module.JourneyError):
        check({"count": 1})
    module().check_unread_count({"count": 3})
    with pytest.raises(module.JourneyError):
        module().check_unread_count({"count": "3"})


def test_alert_projection_keeps_python_decimal_text_and_boolean_state():
    module().check_alert_list([alert()])
    module().check_alert(alert())
    with pytest.raises(module.JourneyError):
        module().check_alert(alert(is_active="true"))
    with pytest.raises(module.JourneyError):
        module().check_alert(alert(threshold_usd=812.5))
    with pytest.raises(module.JourneyError):
        module().check_alert(alert(direction="sideways"))


def test_leaderboard_requires_integer_rank_and_count():
    check = module().check_leaderboard
    check([{"rank": 1, "user_name": "A", "organization_name": None, "referral_count": 2}])
    for bad in (
        {"rank": True, "user_name": "A", "organization_name": None, "referral_count": 2},
        {"rank": 1, "user_name": "A", "organization_name": None, "referral_count": "2"},
        {"rank": 1, "user_name": 3, "organization_name": None, "referral_count": 2},
    ):
        with pytest.raises(module.JourneyError):
            check([bad])


def test_watchlist_event_page_and_summary_require_the_rendered_fields():
    module().check_events_page({"items": [], "next_cursor": None})
    with pytest.raises(module.JourneyError):
        module().check_events_page({"items": [], "next_cursor": 3})
    with pytest.raises(module.JourneyError):
        module().check_events_page({"items": {}})
    module().check_watchlist_summary({
        "id": "10000000-0000-4000-8000-000000000001",
        "name": "Market Radar",
        "kind": "RADAR_DEFAULT",
        "unread_event_count": 0,
        "latest_event_at": None,
        "total_slice_count": 0,
        "has_more_slices": False,
        "slices": [],
        "created_at": "2026-10-19T10:00:00Z",
    })
    with pytest.raises(module.JourneyError):
        module().check_watchlist_summary({"id": "x"})


def test_journey_target_must_be_loopback_with_the_disposable_token():
    module_ = module()
    module_.validate_target("http://127.0.0.1:49152", "x" * 32)
    module_.validate_target("http://localhost:49152", "x" * 32)
    for url, token in (("https://api.verdaxis.exchange", "x" * 32),
                       ("http://192.168.0.9:49152", "x" * 32),
                       ("http://127.0.0.1:49152", ""),
                       ("http://127.0.0.1:49152", "short")):
        with pytest.raises(module_.JourneyError):
            module_.validate_target(url, token)


def test_journey_call_paths_are_local_absolute_api_paths():
    module_ = module()
    for calls in module_.journeys().values():
        for call in calls:
            assert call.path.startswith("/api/") and not call.path.startswith("//")
            assert call.method in {"GET", "POST", "PUT", "PATCH", "DELETE"}
