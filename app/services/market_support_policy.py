"""Fail-closed runtime policy for the delegated listing pilot."""
from __future__ import annotations

import os
from uuid import UUID


_TRUE_VALUES = frozenset({"1", "true", "yes", "on"})
_FALSE_VALUES = frozenset({"0", "false", "no", "off", ""})


def delegated_listings_enabled() -> bool:
    """Return the kill-switch state; malformed values fail closed."""
    raw = os.getenv("ADMIN_DELEGATED_LISTINGS_ENABLED", "false").strip().lower()
    if raw in _TRUE_VALUES:
        return True
    if raw in _FALSE_VALUES:
        return False
    return False


def delegated_listing_max_ttl_hours() -> int:
    """Bound the pilot order lifetime even when an authorization is broader."""
    raw = os.getenv("ADMIN_DELEGATED_LISTINGS_MAX_TTL_HOURS", "168").strip()
    try:
        value = int(raw)
    except ValueError:
        return 168
    return min(max(value, 1), 720)


def bootstrap_authorization_admin_ids() -> frozenset[UUID]:
    """IDs allowed to create the first durable capability grants.

    This is intentionally an explicit deployment allowlist rather than an
    ADMIN-role fallback.  Once grants exist, routine authorization management
    is database-backed and immediately revocable.
    """
    values: set[UUID] = set()
    for raw in os.getenv("MARKET_SUPPORT_BOOTSTRAP_ADMIN_USER_IDS", "").split(","):
        candidate = raw.strip()
        if not candidate:
            continue
        try:
            values.add(UUID(candidate))
        except ValueError:
            # Fail closed for malformed entries; never broaden the allowlist.
            continue
    return frozenset(values)
