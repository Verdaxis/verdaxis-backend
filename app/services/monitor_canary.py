"""Shared monitor canary predicates."""

from __future__ import annotations

CANARY_EMAIL_SUFFIX = ".canary.verdaxis.exchange"


def is_monitor_canary_email(email: str) -> bool:
    local, _, domain = email.partition("@")
    return local.startswith("canary+") and domain.endswith(CANARY_EMAIL_SUFFIX)


def get_monitor_canary_domain(email: str) -> str | None:
    if not is_monitor_canary_email(email):
        return None
    return email.partition("@")[2]
