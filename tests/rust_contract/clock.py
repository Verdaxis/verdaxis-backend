"""Fixed-clock injection for the disposable Python oracle.

A full oracle run must observe one explicit instant so clock-dependent reads
are reproducible. The wrapper patches the exact symbols the ported routes bind
for the current time. The manifests below name every module and symbol this
build expects; a source change that moves or removes one makes
``test_clock.py`` fail instead of letting a guarded patch silently skip a
clock. Never patch a symbol this manifest does not name.
"""

from __future__ import annotations

import importlib
import os
from contextlib import ExitStack
from datetime import UTC, date, datetime
from unittest.mock import patch

# Modules that bind the `datetime` class directly and use it for cutoffs,
# generation times, certificate checks, or expiry comparisons.
DATETIME_MODULES = (
    "app.services.market_data_eligibility",
    "app.services.fame_order",
    "app.services.live_benchmarks",
    "app.services.forward_curve_market_slices",
    "app.services.forward_monitoring",
    "app.services.availability_windows",
    "app.schemas.curves",
    "app.routers.orderbook",
    "app.routers.curves",
    "app.routers.price_discovery",
    "app.routers.trade_tape",
    "app.mcp_server",
)

# Modules that bind `date` and call `date.today()` for window aliases. Every
# `date.today()` call site in a ported module must appear here.
DATE_MODULES = (
    "app.services.availability_windows",
    "app.routers.orderbook",
)

# Modules that import and call `current_public_order_clause` for public scope.
CURRENT_ORDER_CLAUSE_MODULES = (
    "app.services.market_data_eligibility",
    "app.services.public_order_scope",
    "app.services.live_benchmarks",
    "app.routers.curves",
    "app.routers.inventory",
    "app.services.forward_curve_market_slices",
)
CURRENT_ORDER_CLAUSE_SYMBOL = "current_public_order_clause"

# The module that binds `kyc_execution_clause` for owner KYC admission.
KYC_CLAUSE_MODULE = "app.services.market_data_eligibility"
KYC_CLAUSE_SYMBOL = "kyc_execution_clause"


def module(name: str):
    """Import a manifest module by dotted name."""
    return importlib.import_module(name)


def manifest_gaps() -> list[str]:
    """Return every (module, symbol) the manifest names but the source lacks."""
    gaps: list[str] = []
    for name in DATETIME_MODULES:
        if not hasattr(module(name), "datetime"):
            gaps.append(f"{name}:datetime")
    for name in DATE_MODULES:
        if not hasattr(module(name), "date"):
            gaps.append(f"{name}:date")
    for name in CURRENT_ORDER_CLAUSE_MODULES:
        if not hasattr(module(name), CURRENT_ORDER_CLAUSE_SYMBOL):
            gaps.append(f"{name}:{CURRENT_ORDER_CLAUSE_SYMBOL}")
    if not hasattr(module(KYC_CLAUSE_MODULE), KYC_CLAUSE_SYMBOL):
        gaps.append(f"{KYC_CLAUSE_MODULE}:{KYC_CLAUSE_SYMBOL}")
    return gaps


def require_test_environment() -> None:
    """Refuse a clock override outside the disposable test environment."""
    if os.environ.get("ENVIRONMENT") != "test":
        raise RuntimeError("clock overrides require ENVIRONMENT=test")


class ActionValues:
    """Test-only action clock and finite generated-value sequences."""
    def __init__(self, now, identifiers, referral_codes):
        require_test_environment()
        if now.tzinfo is None:
            raise ValueError("action clock must be aware")
        self.now = now
        self.identifiers = iter(identifiers)
        self.referral_codes = iter(referral_codes)

    def next_id(self):
        return next(self.identifiers)

    def next_referral_code(self):
        return next(self.referral_codes)

    def advance(self, delta):
        self.now += delta


def fixed_clock_patches(now: datetime) -> ExitStack:
    """Patch every manifest clock symbol to the frozen instant `now`.

    The returned stack is an active context manager: use it with `with` or
    `stack.close()`. The instant must be timezone-aware.
    """
    if now.tzinfo is None:
        raise ValueError("the frozen instant must be timezone-aware")
    require_test_environment()
    from app.models.user import User

    stack = ExitStack()

    class FrozenDatetime(datetime):
        @classmethod
        def now(cls, tz=None):
            return now.astimezone(tz) if tz else now.replace(tzinfo=None)

    class FrozenDate(date):
        @classmethod
        def today(cls):
            return now.astimezone(UTC).date()

    for name in DATETIME_MODULES:
        stack.enter_context(patch.object(module(name), "datetime", FrozenDatetime))
    for name in DATE_MODULES:
        stack.enter_context(patch.object(module(name), "date", FrozenDate))

    eligibility = module("app.services.market_data_eligibility")
    original_current = eligibility.current_public_order_clause

    def current(order, *, now_expression=None):
        return original_current(order, now_expression=now)

    for name in CURRENT_ORDER_CLAUSE_MODULES:
        stack.enter_context(
            patch.object(module(name), CURRENT_ORDER_CLAUSE_SYMBOL, current)
        )

    kyc_module = module(KYC_CLAUSE_MODULE)
    original_kyc = kyc_module.kyc_execution_clause

    def kyc(user=User, *, now=None):
        return original_kyc(user, now=now_frozen)

    now_frozen = now
    stack.enter_context(patch.object(kyc_module, KYC_CLAUSE_SYMBOL, kyc))
    return stack
