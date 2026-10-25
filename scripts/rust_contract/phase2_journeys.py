"""HTTP client journeys for the six Phase 2 CRUD groups.

The call sequences come from `verdaxis-frontend/src/services/api.ts` and the
components that use it: NotificationContext polls the list and the unread
count, `useServerPreference` reads once and writes per namespace, the feedback
button submits and the admin page reads, `useWatchlist` drives the radar and
its targets, TradeHistoryPage lists/creates/deletes alerts, and ReferralsTab
reads the code, statistics and leaderboard before inviting.

The journey asserts the fields, types and status codes those clients read. It
runs against the Rust candidate on a disposable stack; it is not a browser run
and it does not compare with Python. The twin-stack scenarios own the Python
comparison.
"""
from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import UTC, datetime, timedelta
from pathlib import Path

from scripts.rust_contract.phase2_mail import MailStub

ROOT = Path(__file__).resolve().parents[2]
SECRET = "test-secret-key-for-testing-minimum-32-chars"
ISSUER = "verdaxis-test-api"
AUDIENCE = "verdaxis-test-web"

# Disposable fixture identities. No value is a credential.
ORGANIZATION = "10000000-0000-4000-8000-000000000001"
FOREIGN_ORGANIZATION = "10000000-0000-4000-8000-000000000002"
OWNER = "20000000-0000-4000-8000-000000000001"
ADMIN = "20000000-0000-4000-8000-00000000000a"
FOREIGN = "20000000-0000-4000-8000-00000000000f"
RADAR = "70000000-0000-4000-8000-000000000001"
SLICE = "72000000-0000-4000-8000-000000000001"
EVENT = "73000000-0000-4000-8000-000000000001"
PUBLIC_ORDER = "74000000-0000-4000-8000-000000000001"
DEMO_ORGANIZATION = "4da7b285-34ee-5443-9406-f96b4ed1a251"
PRODUCT = "c4cecebc-3d2d-5840-8021-57a9a11bc673"  # B30
DALIAN = "78281fb3-e726-5396-802e-34b01fda21e8"
REFERRAL_CODE = "VDX-JRN001"
BASE = datetime(2026, 10, 19, 10, 0, tzinfo=UTC)


class JourneyError(RuntimeError):
    pass


@dataclass(frozen=True)
class JourneyCall:
    group: str
    name: str
    method: str
    path: str
    status: int = 200
    json_body: object | None = None
    actor: str = "owner"
    check: object | None = None
    query: dict = field(default_factory=dict)
    placeholders: tuple[str, ...] = ()
    # (JourneyState attribute, response key). A list body reads the first item.
    capture: tuple[str, str] | None = None


@dataclass
class JourneyState:
    notification_id: str = ""
    watchlist_id: str = RADAR
    event_id: str = EVENT
    target_id: str = ""
    custom_id: str = ""
    alert_id: str = ""


# ---------------------------------------------------------------------------
# Type checks: the exact shapes the frontend reads.
# ---------------------------------------------------------------------------


def require(condition, message):
    if not condition:
        raise JourneyError(message)


UUID_PATTERN = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
DECIMAL_PATTERN = re.compile(r"-?[0-9]+(\.[0-9]+)?")


def uuid_text(value, name):
    require(isinstance(value, str) and UUID_PATTERN.fullmatch(value) is not None,
            f"{name} is not a UUID string")
    return value


def instant(value, name):
    require(isinstance(value, str), f"{name} is not a string instant")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError as error:
        raise JourneyError(f"{name} is not an ISO-8601 instant") from error
    require(parsed.tzinfo is not None, f"{name} is not timezone aware")
    return parsed


def integer(value, name):
    require(type(value) is int, f"{name} is not an integer")
    return value


def boolean(value, name):
    require(type(value) is bool, f"{name} is not a boolean")
    return value


def text(value, name):
    require(isinstance(value, str), f"{name} is not a string")
    return value


def nullable(value, name, kind):
    if value is None:
        return None
    return kind(value, name)


def mapping(value, name):
    require(isinstance(value, dict), f"{name} is not an object")
    return value


def decimal_text(value, name):
    require(isinstance(value, str) and DECIMAL_PATTERN.fullmatch(value) is not None,
            f"{name} is not decimal text")
    return value


REQUIRED_NOTIFICATION = {"id", "type", "title", "message", "data", "is_read", "created_at"}
NOTIFICATION_TYPES = {"SYSTEM", "ORDER_UPDATE", "DIRECT_ORDER", "DIRECT_ORDER_OFFER", "USER_STATUS"}


def check_notification_list(value):
    require(isinstance(value, list), "notification list is not an array")
    for item in value:
        mapping(item, "notification")
        require(REQUIRED_NOTIFICATION <= set(item), "notification field set is incomplete")
        uuid_text(item["id"], "notification.id")
        require(item["type"] in NOTIFICATION_TYPES, "notification.type is not a declared enum")
        text(item["title"], "notification.title")
        text(item["message"], "notification.message")
        nullable(item["data"], "notification.data", mapping)
        boolean(item["is_read"], "notification.is_read")
        instant(item["created_at"], "notification.created_at")


def check_unread_count(value):
    mapping(value, "unread count")
    require(set(value) == {"count"}, "unread count fields differ")
    integer(value["count"], "count")


def check_marked_read(value):
    mapping(value, "mark-read result")
    require(value == {"status": "success"}, "mark-read result differs")


def check_preferences_map(value):
    return mapping(value, "preferences")


def check_preference_write(expected):
    def check(value):
        mapping(value, "preference write")
        require(value.get("value") == expected, "preference write did not echo the value")
        instant(value.get("updated_at"), "preference.updated_at")
    return check


def check_feedback_created(value):
    mapping(value, "feedback create")
    uuid_text(value.get("id"), "feedback.id")
    instant(value.get("created_at"), "feedback.created_at")
    require(set(value) == {"id", "created_at"}, "feedback create fields differ")


def check_admin_feedback(value):
    mapping(value, "admin feedback")
    require(set(value) == {"items", "total"}, "admin feedback fields differ")
    integer(value["total"], "feedback.total")
    require(isinstance(value["items"], list), "feedback items is not an array")
    require(len(value["items"]) <= value["total"], "feedback page exceeds its total")
    for item in value["items"]:
        mapping(item, "feedback item")
        require({"id", "created_at", "message", "page", "user_email", "user_name", "org_name"} <= set(item),
                "feedback item field set is incomplete")
        uuid_text(item["id"], "feedback item.id")
        instant(item["created_at"], "feedback item.created_at")
        text(item["message"], "feedback item.message")
        for name in ("page", "user_email", "user_name", "org_name"):
            nullable(item[name], "feedback item." + name, text)


WATCHLIST_SUMMARY_FIELDS = {"id", "name", "kind", "unread_event_count", "latest_event_at",
                            "total_slice_count", "has_more_slices", "slices", "created_at"}
WATCHLIST_SLICE_FIELDS = {"id", "target_type", "market_product_code", "delivery_point_id",
                          "delivery_point_name", "availability_window_code", "active_order_count",
                          "unread_event_count", "latest_event_at", "pins", "created_at"}
WATCHLIST_TARGET_FIELDS = {"id", "target_type", "market_product_code", "delivery_point_id",
                           "delivery_point_name", "availability_window_code", "order_id",
                           "snapshot_price_per_mt_usd", "snapshot_quantity_mt",
                           "snapshot_remaining_quantity_mt", "snapshot_status", "snapshot_side",
                           "snapshot_market_product", "snapshot_delivery_point_name",
                           "snapshot_availability_window", "snapshot_counterparty_label",
                           "active_order_count", "unread_event_count", "latest_event_at", "created_at"}


def check_watchlist(value):
    mapping(value, "watchlist")
    require({"id", "name", "entry_count", "entries", "created_at"} <= set(value),
            "watchlist field set is incomplete")
    uuid_text(value["id"], "watchlist.id")
    text(value["name"], "watchlist.name")
    integer(value["entry_count"], "watchlist.entry_count")
    require(isinstance(value["entries"], list), "watchlist entries is not an array")
    instant(value["created_at"], "watchlist.created_at")


def check_watchlist_target(value):
    mapping(value, "watchlist target")
    require(WATCHLIST_TARGET_FIELDS <= set(value), "watchlist target field set is incomplete")
    uuid_text(value["id"], "target.id")
    require(value["target_type"] in {"SLICE", "PIN"}, "target.target_type is not declared")
    for name in ("market_product_code", "delivery_point_id", "delivery_point_name",
                 "availability_window_code", "order_id", "snapshot_status", "snapshot_side",
                 "snapshot_market_product", "snapshot_delivery_point_name",
                 "snapshot_availability_window", "snapshot_counterparty_label"):
        nullable(value[name], "target." + name, text)
    for name in ("snapshot_price_per_mt_usd", "snapshot_quantity_mt", "snapshot_remaining_quantity_mt"):
        require(value[name] is None or type(value[name]) in (int, float), f"target.{name} is not numeric")
    integer(value["active_order_count"], "target.active_order_count")
    integer(value["unread_event_count"], "target.unread_event_count")
    nullable(value["latest_event_at"], "target.latest_event_at", instant)
    instant(value["created_at"], "target.created_at")


def check_watchlist_summary(value):
    mapping(value, "watchlist summary")
    require(WATCHLIST_SUMMARY_FIELDS <= set(value), "watchlist summary field set is incomplete")
    uuid_text(value["id"], "watchlist.id")
    text(value["name"], "watchlist.name")
    text(value["kind"], "watchlist.kind")
    integer(value["unread_event_count"], "watchlist.unread_event_count")
    integer(value["total_slice_count"], "watchlist.total_slice_count")
    boolean(value["has_more_slices"], "watchlist.has_more_slices")
    nullable(value["latest_event_at"], "watchlist.latest_event_at", instant)
    instant(value["created_at"], "watchlist.created_at")
    require(isinstance(value["slices"], list), "watchlist.slices is not an array")
    for slice_ in value["slices"]:
        mapping(slice_, "watchlist slice")
        require(WATCHLIST_SLICE_FIELDS <= set(slice_), "watchlist slice field set is incomplete")
        uuid_text(slice_["id"], "slice.id")
        require(slice_["target_type"] == "SLICE", "slice.target_type is not SLICE")
        text(slice_["market_product_code"], "slice.market_product_code")
        text(slice_["delivery_point_id"], "slice.delivery_point_id")
        text(slice_["availability_window_code"], "slice.availability_window_code")
        integer(slice_["active_order_count"], "slice.active_order_count")
        integer(slice_["unread_event_count"], "slice.unread_event_count")
        nullable(slice_["latest_event_at"], "slice.latest_event_at", instant)
        instant(slice_["created_at"], "slice.created_at")
        require(isinstance(slice_["pins"], list), "slice.pins is not an array")
        for pin in slice_["pins"]:
            check_watchlist_target(pin)


EVENT_FIELDS = {"id", "watchlist_id", "watchlist_target_id", "target_type", "event_type",
                "event_payload", "source_kind", "scope", "demo_status", "observed_at",
                "market_product_code", "delivery_point_id", "delivery_point_name",
                "availability_window_code", "order_id", "is_read", "created_at"}


def check_watchlist_event(value):
    mapping(value, "watchlist event")
    require(EVENT_FIELDS <= set(value), "watchlist event field set is incomplete")
    uuid_text(value["id"], "event.id")
    uuid_text(value["watchlist_id"], "event.watchlist_id")
    uuid_text(value["watchlist_target_id"], "event.watchlist_target_id")
    require(value["target_type"] in {"SLICE", "PIN"}, "event.target_type is not declared")
    text(value["event_type"], "event.event_type")
    mapping(value["event_payload"], "event.event_payload")
    for name in ("source_kind", "scope", "demo_status"):
        text(value[name], "event." + name)
    nullable(value["observed_at"], "event.observed_at", instant)
    for name in ("market_product_code", "delivery_point_id", "delivery_point_name",
                 "availability_window_code", "order_id"):
        nullable(value[name], "event." + name, text)
    boolean(value["is_read"], "event.is_read")
    instant(value["created_at"], "event.created_at")


def check_events_page(value):
    mapping(value, "events page")
    require(set(value) == {"items", "next_cursor"}, "events page fields differ")
    require(isinstance(value["items"], list), "events page items is not an array")
    nullable(value["next_cursor"], "events page next_cursor", text)
    for item in value["items"]:
        check_watchlist_event(item)


ALERT_FIELDS = {"id", "org_id", "product_id", "product_name", "market_product",
                "delivery_point_id", "direction", "threshold_usd", "is_active",
                "triggered_at", "created_at"}


def check_alert(value):
    mapping(value, "alert")
    require(ALERT_FIELDS <= set(value), "alert field set is incomplete")
    uuid_text(value["id"], "alert.id")
    uuid_text(value["org_id"], "alert.org_id")
    uuid_text(value["product_id"], "alert.product_id")
    nullable(value["product_name"], "alert.product_name", text)
    nullable(value["market_product"], "alert.market_product", text)
    nullable(value["delivery_point_id"], "alert.delivery_point_id", text)
    require(value["direction"] in {"above", "below"}, "alert.direction is not declared")
    decimal_text(value["threshold_usd"], "alert.threshold_usd")
    boolean(value["is_active"], "alert.is_active")
    nullable(value["triggered_at"], "alert.triggered_at", instant)
    instant(value["created_at"], "alert.created_at")


def check_alert_list(value):
    require(isinstance(value, list), "alert list is not an array")
    for item in value:
        check_alert(item)


def check_referral_code(value):
    mapping(value, "referral code")
    require(set(value) == {"referral_code", "referral_link"}, "referral code fields differ")
    require(re.fullmatch(r"VDX-[A-Z0-9]{6}", text(value["referral_code"], "referral_code")) is not None,
            "referral_code is not VDX-format")
    require(text(value["referral_link"], "referral_link").startswith(("http://", "https://")),
            "referral_link is not absolute")


def check_referral_stats(value):
    mapping(value, "referral stats")
    require(set(value) == {"total", "verified", "active", "referrals"}, "referral stats fields differ")
    for name in ("total", "verified", "active"):
        integer(value[name], "referral." + name)
    require(isinstance(value["referrals"], list), "referral.referrals is not an array")
    require(value["verified"] <= value["total"] and value["active"] <= value["verified"],
            "referral counts are not ordered")
    for item in value["referrals"]:
        mapping(item, "referral item")
        require(set(item) == {"organization_name", "role", "status", "signed_up_at"},
                "referral item fields differ")
        nullable(item["organization_name"], "referral.organization_name", text)
        nullable(item["role"], "referral.role", text)
        text(item["status"], "referral.status")
        instant(item["signed_up_at"], "referral.signed_up_at")


def check_leaderboard(value):
    require(isinstance(value, list), "leaderboard is not an array")
    require(len(value) <= 10, "leaderboard is longer than ten")
    counts = []
    for rank, item in enumerate(value, start=1):
        mapping(item, "leaderboard entry")
        require(set(item) == {"rank", "user_name", "organization_name", "referral_count"},
                "leaderboard entry fields differ")
        require(type(item["rank"]) is int and item["rank"] == rank, "leaderboard rank is not consecutive")
        text(item["user_name"], "leaderboard.user_name")
        nullable(item["organization_name"], "leaderboard.organization_name", text)
        integer(item["referral_count"], "leaderboard.referral_count")
        require(item["referral_count"] > 0, "leaderboard count is not positive")
        counts.append(item["referral_count"])
    require(counts == sorted(counts, reverse=True), "leaderboard counts are not descending")


def check_invite(value):
    mapping(value, "invite")
    require(set(value) == {"message", "email"}, "invite fields differ")
    text(value["message"], "invite.message")
    text(value["email"], "invite.email")


def check_resolve(value):
    mapping(value, "resolve")
    require(set(value) == {"valid", "organization_name", "organization_type", "referrer_name"},
            "resolve fields differ")
    boolean(value["valid"], "resolve.valid")
    for name in ("organization_name", "organization_type", "referrer_name"):
        nullable(value[name], "resolve." + name, text)


def check_configured_namespace(value):
    check_preferences_map(value)
    require("tutorial" in value, "preferences did not return the written namespace")


# ---------------------------------------------------------------------------
# The six journeys, in the order the frontend issues them.
# ---------------------------------------------------------------------------


def journeys() -> dict[str, tuple[JourneyCall, ...]]:
    def call(group, name, method, path, status=200, body=None, actor="owner", check=None,
             query=None, placeholders=(), capture=None):
        return JourneyCall(group, name, method, path, status, body, actor, check,
                           query or {}, placeholders, capture)

    notifications = (
        call("notifications", "list", "GET", "/api/notifications", check=check_notification_list,
             capture=("notification_id", "id")),
        call("notifications", "unread-count", "GET", "/api/notifications/unread-count",
             check=check_unread_count),
        call("notifications", "mark-read", "PATCH", "/api/notifications/{notification_id}/read",
             check=check_marked_read, placeholders=("notification_id",)),
        call("notifications", "read-all", "PATCH", "/api/notifications/read-all",
             check=check_marked_read),
        call("notifications", "unread-count-after", "GET", "/api/notifications/unread-count",
             check=check_unread_count),
    )
    preferences = (
        call("preferences", "read-all", "GET", "/api/users/me/preferences", check=check_preferences_map),
        call("preferences", "put-tutorial", "PUT", "/api/users/me/preferences/tutorial",
             body={"completed": True}, check=check_preference_write({"completed": True})),
        call("preferences", "read-back", "GET", "/api/users/me/preferences",
             check=check_configured_namespace),
    )
    feedback = (
        call("feedback", "submit", "POST", "/api/feedback", 201,
             body={"message": "Journey feedback", "page": "/app/trades"}, check=check_feedback_created),
        call("feedback", "admin-list", "GET", "/api/admin/feedback", actor="admin",
             query={"limit": "100", "offset": "0"}, check=check_admin_feedback),
    )
    watchlists = (
        call("watchlists", "radar", "POST", "/api/watchlists/me", check=check_watchlist_summary,
             capture=("watchlist_id", "id")),
        call("watchlists", "events", "GET", "/api/watchlists/{watchlist_id}/events",
             query={"limit": "25"}, check=check_events_page,
             placeholders=("watchlist_id",), capture=("event_id", "items/0/id")),
        call("watchlists", "mark-event-read", "PATCH",
             "/api/watchlists/{watchlist_id}/events/{event_id}", check=check_watchlist_event,
             placeholders=("watchlist_id", "event_id")),
        call("watchlists", "add-slice", "POST", "/api/watchlists/{watchlist_id}/targets", 201,
             body={"target_type": "SLICE", "market_product_code": "B30",
                   "delivery_point_id": DALIAN, "availability_window_code": "2026-12"},
             check=check_watchlist_target, placeholders=("watchlist_id",),
             capture=("target_id", "id")),
        call("watchlists", "add-pin", "POST", "/api/watchlists/{watchlist_id}/targets", 201,
             body={"target_type": "PIN", "order_id": PUBLIC_ORDER},
             check=check_watchlist_target, placeholders=("watchlist_id",),
             capture=("target_id", "id")),
        call("watchlists", "remove-target", "DELETE",
             "/api/watchlists/{watchlist_id}/targets/{target_id}", 204,
             placeholders=("watchlist_id", "target_id")),
        call("watchlists", "create-list", "POST", "/api/watchlists", 201,
             body={"name": "Journey list"}, check=check_watchlist, capture=("custom_id", "id")),
        # W09/W10 (legacy entries) stay on Python in Phase 2, so the Rust
        # journey stops at the container routes the frontend shares with it.
        call("watchlists", "delete-list", "DELETE", "/api/watchlists/{custom_id}", 204,
             placeholders=("custom_id",)),
    )
    alerts = (
        call("alerts", "list-empty", "GET", "/api/alerts", check=check_alert_list),
        call("alerts", "create", "POST", "/api/alerts", 201,
             body={"product_id": PRODUCT, "direction": "above", "threshold_usd": 900},
             check=check_alert, capture=("alert_id", "id")),
        call("alerts", "list-after-create", "GET", "/api/alerts", check=check_alert_list),
        call("alerts", "delete", "DELETE", "/api/alerts/{alert_id}", 204, placeholders=("alert_id",)),
        call("alerts", "list-after-delete", "GET", "/api/alerts", check=check_alert_list),
    )
    referrals = (
        call("referrals", "my-code", "POST", "/api/referrals/my-code", check=check_referral_code),
        call("referrals", "my-referrals", "GET", "/api/referrals/my-referrals",
             check=check_referral_stats),
        call("referrals", "leaderboard", "GET", "/api/referrals/leaderboard", check=check_leaderboard),
        # pydantic's email validator refuses special-use domains such as
        # `.invalid`; the invite address uses an ordinary example domain.
        call("referrals", "invite", "POST", "/api/referrals/invite",
             body={"email": "journey-invite@example.com"}, check=check_invite),
        call("referrals", "resolve", "GET", "/api/referrals/resolve/" + REFERRAL_CODE,
             actor="none", check=check_resolve),
    )
    return {
        "notifications": notifications,
        "preferences": preferences,
        "feedback": feedback,
        "watchlists": watchlists,
        "alerts": alerts,
        "referrals": referrals,
    }


def _captured(body, key):
    """Read one value from a response body; a `/`-path walks into pages.

    A list step without a numeric segment reads the first item, so a plain
    `id` capture keeps its meaning on a list body.
    """
    for segment in key.split("/"):
        if isinstance(body, list):
            if segment.isdigit():
                index = int(segment)
                require(0 <= index < len(body), f"captured path {key} is out of range")
                body = body[index]
                continue
            require(body, "cannot capture from an empty list")
            body = body[0]
        mapping(body, "captured response")
        require(body.get(segment), f"captured field {key} is missing")
        body = body[segment]
    return body


def validate_target(base_url: str, token: str) -> None:
    from urllib.parse import urlsplit

    parsed = urlsplit(base_url)
    require(parsed.scheme == "http" and parsed.hostname in {"127.0.0.1", "localhost"},
            "journeys run only against a loopback disposable target")
    require(parsed.port is not None, "journey target needs an explicit port")
    require(isinstance(token, str) and len(token) >= 32, "disposable token is required")


# ---------------------------------------------------------------------------
# Fixtures and execution.
# ---------------------------------------------------------------------------


def setup(connection) -> None:
    """Rows the six journeys need; the base seed supplies products and points."""
    from scripts.rust_contract.phase2_watchlist_fixtures import (
        ORGANIZATION,
        insert_container,
        insert_event,
        insert_slice,
        order_event,
        seed_users_and_orders,
    )

    # The order-provenance guard reads the owning organization with SELECT INTO
    # STRICT, and organizations.provenance defaults to UNKNOWN, so the shared
    # fixture's UNKNOWN-provenance order needs this row present before it. The
    # scenario path seeds the same row into its clone template.
    connection.execute(
        "INSERT INTO organizations(id,name,domain,type) VALUES (%s,'Journey org','journey.invalid','FUEL_BUYER')",
        (ORGANIZATION,))
    seed_users_and_orders(connection)
    connection.execute(
        "INSERT INTO organizations(id,name,domain,type) VALUES (%s,'Foreign journey org','foreign.journey.invalid','FUEL_BUYER')",
        (FOREIGN_ORGANIZATION,))
    for user, email, role, organization in (
            (OWNER, "journey-owner@example.invalid", "BUYER", ORGANIZATION),
            (ADMIN, "journey-admin@example.invalid", "ADMIN", ORGANIZATION),
            (FOREIGN, "journey-foreign@example.invalid", "BUYER", FOREIGN_ORGANIZATION)):
        connection.execute(
            "INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,"
            "must_change_password,first_name,last_name,referral_code) "
            "VALUES (%s,%s,'not-a-login',%s,'APPROVED',%s,true,false,'Journey','User',%s)",
            (user, email, role, organization, REFERRAL_CODE if user == OWNER else None))
    # PUBLIC_ORDER comes from seed_users_and_orders; the pin below reads it.
    insert_container(connection, RADAR, OWNER, "Market Radar", BASE, kind="RADAR_DEFAULT")
    insert_slice(connection, SLICE, RADAR, "B30", DALIAN, "SPOT", BASE)
    # No payload order binding: the W07 visibility guard treats a bound
    # source order as gated until proven visible, which would drop the page's
    # only event under the journey server's default admission snapshot.
    insert_event(connection, EVENT, RADAR, SLICE, "PIN_PRICE_CHANGED", order_event(None), BASE)
    for index in range(2):
        connection.execute(
            "INSERT INTO notifications(id,recipient_id,type,title,message,data,is_read,created_at) "
            "VALUES (%s,%s,'SYSTEM','Title','Message',NULL,%s,%s)",
            (f"31000000-0000-4000-8000-{index:012d}", OWNER, index == 1, BASE))
    # One referral per status so the settings tab renders its table, counts and
    # a leaderboard row.
    for index, (status, organization) in enumerate(
            (("VERIFIED", FOREIGN_ORGANIZATION), ("ACTIVE", None)), start=1):
        user = f"21000000-0000-4000-8000-{index:012d}"
        connection.execute(
            "INSERT INTO users(id,email,password_hash,role,status,organization_id,email_verified,must_change_password) "
            "VALUES (%s,%s,'not-a-login','BUYER','APPROVED',%s,true,false)",
            (user, f"referred-{index}@example.invalid", organization))
        connection.execute(
            "INSERT INTO referrals(id,referrer_id,referred_user_id,referral_code_used,status,created_at) "
            "VALUES (%s,%s,%s,%s,%s,%s)",
            (f"34000000-0000-4000-8000-{index:012d}", OWNER, user, REFERRAL_CODE, status, BASE))


def _token(actor: str) -> str:
    import jwt

    now = datetime.now(UTC)
    claims = {"sub": actor, "type": "access", "iat": int(now.timestamp()),
              "exp": int((now + timedelta(hours=2)).timestamp()),
              "jti": "journey", "iss": ISSUER, "aud": AUDIENCE}
    return jwt.encode(claims, SECRET, algorithm="HS256")


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def _request_path(call: JourneyCall, state: JourneyState) -> str:
    try:
        path = call.path.format(
            notification_id=state.notification_id,
            watchlist_id=state.watchlist_id,
            event_id=state.event_id,
            target_id=state.target_id,
            custom_id=state.custom_id,
            alert_id=state.alert_id,
        )
    except KeyError as error:
        raise JourneyError(f"unresolved journey placeholder {error}") from error
    if not path.startswith("/api/") or path.startswith("//") or "//" in path:
        raise JourneyError("journey path must be a local API path")
    if call.query:
        from urllib.parse import urlencode
        path += "?" + urlencode(call.query)
    return path


def send(endpoint: str, token: str, call: JourneyCall, state: JourneyState):
    path = _request_path(call, state)
    headers = {"content-type": "application/json", "x-disposable-token": token,
               "x-request-id": "journey-" + call.name}
    if call.actor == "owner":
        headers["authorization"] = "Bearer " + _token(OWNER)
    elif call.actor == "admin":
        headers["authorization"] = "Bearer " + _token(ADMIN)
    data = None if call.json_body is None else json.dumps(call.json_body).encode()
    request = urllib.request.Request(endpoint + path, method=call.method, headers=headers, data=data)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect())
    try:
        response = opener.open(request, timeout=20)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(8 * 1024 * 1024 + 1)
        if len(raw) > 8 * 1024 * 1024:
            raise JourneyError("journey response exceeds the capture bound")
        body = None
        if response.status not in (204, 205) and raw:
            body = json.loads(raw)
        return response.status, body


def run_journeys(endpoint: str, token: str, groups=None) -> dict:
    """Execute the journeys and return a report; every failure is collected."""
    validate_target(endpoint, token)
    report = {"endpoint": endpoint, "groups": [], "passed": True}
    selected = journeys()
    for group in groups or list(selected):
        state = JourneyState()
        group_report = {"group": group, "calls": [], "passed": True}
        for call in selected[group]:
            try:
                status, body = send(endpoint, token, call, state)
                if status != call.status:
                    raise JourneyError(f"status {status} != {call.status}")
                if call.check is not None:
                    call.check(body)
                if call.capture is not None:
                    setattr(state, call.capture[0], _captured(body, call.capture[1]))
                group_report["calls"].append({"name": call.name, "status": status, "fields": True})
            except (JourneyError, ValueError, KeyError, TypeError, OSError) as error:
                group_report["calls"].append({"name": call.name, "error": str(error)})
                group_report["passed"] = False
                report["passed"] = False
                break
        report["groups"].append(group_report)
    return report


def run(runner, args) -> None:
    """Runner-owned disposable lifecycle for the client journeys."""
    from scripts.rust_contract.phase2_seed import (
        APP_ROLE,
        BACKUP_ROLE,
        MIGRATOR_ROLE,
        PASSWORD,
        connect,
    )
    from scripts.rust_contract.run_disposable import (
        DB_NAME,
        attest,
        docker,
        ephemeral_port,
        wait_for_port,
    )

    port = runner.start_database()
    policy_dir = "/tmp/phase2-journey-policy"
    docker("exec", runner.container, "mkdir", "-p", policy_dir)
    for name in ("app_acl_policy.sql", "converge_runtime_object_acls.sql",
                 "bootstrap_roles.sql", "validate_roles.sql"):
        docker("cp", str(ROOT / "deploy/postgres" / name), f"{runner.container}:{policy_dir}/{name}")
    # Before the migration only the roles exist. `validate_roles.sql` asserts
    # the migrator owns `alembic_version`, so it runs after `migrate_and_seed`,
    # exactly as the scenario path orders it.
    docker("exec", runner.container, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "postgres",
           "-d", DB_NAME, "-v", f"database_name={DB_NAME}", "-v", f"app_role={APP_ROLE}",
           "-v", f"migrator_role={MIGRATOR_ROLE}", "-v", f"backup_role={BACKUP_ROLE}",
           "-f", f"{policy_dir}/bootstrap_roles.sql")
    with connect(port, DB_NAME) as source:
        source.execute(f"ALTER ROLE \"{MIGRATOR_ROLE}\" PASSWORD '{PASSWORD}'")
    migrator_url = f"postgresql+asyncpg://{MIGRATOR_ROLE}:{PASSWORD}@127.0.0.1:{port}/{DB_NAME}"
    runner.seed = False
    runner.migrate_and_seed({**runner.urls(port), "DATABASE_URL": migrator_url,
                             "MIGRATOR_DATABASE_URL": migrator_url})
    for script in ("bootstrap_roles.sql", "validate_roles.sql"):
        docker("exec", runner.container, "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "postgres",
               "-d", DB_NAME, "-v", f"database_name={DB_NAME}", "-v", f"app_role={APP_ROLE}",
               "-v", f"migrator_role={MIGRATOR_ROLE}", "-v", f"backup_role={BACKUP_ROLE}",
               "-f", f"{policy_dir}/{script}")
    with connect(port, DB_NAME) as source:
        # The server authenticates as the app role with the scenario password;
        # start_database created it with the owner password instead.
        source.execute(f"ALTER ROLE \"{APP_ROLE}\" PASSWORD '{PASSWORD}'")
        setup(source)

    subprocess.run(["cargo", "build", "--manifest-path", "rs/Cargo.toml", "--locked",
                    "--features", "contract-tests", "--example", "phase2_journey_server"],
                   cwd=ROOT, env={**os.environ, "SQLX_OFFLINE": "true"}, check=True)
    bind = ephemeral_port()
    with MailStub() as mail:
        environment = {**os.environ, "ENVIRONMENT": "test", "RELEASE_SHA": runner.sha,
                       "JWT_SECRET": SECRET,
                       "PHASE2_DATABASE_URL": f"postgresql://{APP_ROLE}:{PASSWORD}@127.0.0.1:{port}/{DB_NAME}",
                       "PHASE2_BIND": f"127.0.0.1:{bind}", "DISPOSABLE_TEST_TOKEN": runner.token,
                       "PHASE2_MAIL_URL": f"{mail.url}/journey/emails",
                       "SQLX_OFFLINE": "true"}
        runner.spawn([str(ROOT / "rs/target/debug/examples/phase2_journey_server")], environment)
        wait_for_port(bind)
        endpoint = f"http://127.0.0.1:{bind}"
        attest(endpoint, runner.token)
        report = run_journeys(endpoint, runner.token)
    report.update({"candidate_sha": runner.sha, "utc": datetime.now(UTC).isoformat(),
                   "host": socket.gethostname()})
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise RuntimeError("client journey failed")
