from uuid import uuid4

from app.services.market_support_policy import (
    bootstrap_authorization_admin_ids,
    delegated_listing_max_ttl_hours,
    delegated_listings_enabled,
)


def test_feature_flag_fails_closed(monkeypatch):
    monkeypatch.setenv("ADMIN_DELEGATED_LISTINGS_ENABLED", "unexpected")
    assert delegated_listings_enabled() is False


def test_feature_flag_accepts_explicit_true(monkeypatch):
    monkeypatch.setenv("ADMIN_DELEGATED_LISTINGS_ENABLED", "true")
    assert delegated_listings_enabled() is True


def test_ttl_is_bounded(monkeypatch):
    monkeypatch.setenv("ADMIN_DELEGATED_LISTINGS_MAX_TTL_HOURS", "9999")
    assert delegated_listing_max_ttl_hours() == 720
    monkeypatch.setenv("ADMIN_DELEGATED_LISTINGS_MAX_TTL_HOURS", "0")
    assert delegated_listing_max_ttl_hours() == 1


def test_bootstrap_allowlist_ignores_invalid_ids(monkeypatch):
    expected = uuid4()
    monkeypatch.setenv(
        "MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS",
        f"invalid,{expected}",
    )
    assert bootstrap_authorization_admin_ids() == frozenset({expected})
