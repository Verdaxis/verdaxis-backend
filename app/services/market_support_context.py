"""Request-local attribution for Verdaxis-assisted market actions.

The authenticated Verdaxis administrator remains the audit actor while the
customer user remains the order's accountable execution principal.  A
ContextVar keeps that distinction explicit when the delegated route invokes
existing orderbook application functions inside the same request task.
"""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar, Token
from dataclasses import dataclass
from typing import Iterator
from uuid import UUID


@dataclass(frozen=True)
class MarketSupportActionContext:
    actor_user_id: UUID
    target_organization_id: UUID
    accountable_user_id: UUID
    support_authorization_id: UUID
    operation: str
    reason_code: str
    support_case_reference: str | None
    idempotency_key: str
    request_hash: str
    support_version: int


_market_support_action: ContextVar[MarketSupportActionContext | None] = ContextVar(
    "market_support_action",
    default=None,
)


def current_market_support_action() -> MarketSupportActionContext | None:
    """Return the active delegated-action context for the current async task."""
    return _market_support_action.get()


@contextmanager
def market_support_action_context(
    context: MarketSupportActionContext,
) -> Iterator[MarketSupportActionContext]:
    """Install a delegated-action context for one synchronous/async call tree."""
    token: Token[MarketSupportActionContext | None] = _market_support_action.set(context)
    try:
        yield context
    finally:
        _market_support_action.reset(token)
