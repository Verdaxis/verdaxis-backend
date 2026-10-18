"""Shared-relation handlers keep the planner's JIT policy transaction-local.

The expanded corpus makes the shared eligibility subplans cross PostgreSQL's JIT
cost threshold, so a route that returns a few rows can still pay hundreds of
milliseconds of JIT compilation. The paged orderbook handlers, the listings
handler (R27) and the prices reference aggregate (R14) all begin a transaction
and `SET LOCAL jit = off` before their shared-relation query. This test scans
the route sources so a new handler cannot silently omit the same policy. It
asserts a source property, not a runtime one; the corpus and the latency probe
still own the behaviour.
"""

from pathlib import Path

ROUTES = Path(__file__).resolve().parents[2] / "rs" / "read" / "src" / "routes"

# The handlers that aggregate or page the expanded shared relations.
SHARED_AGGREGATE_HANDLERS = (
    "orderbook_lists.rs",
    "orderbook_map.rs",
    "listings.rs",
    "prices.rs",
)


def test_shared_relation_handlers_disable_jit_locally():
    offenders = [
        name
        for name in SHARED_AGGREGATE_HANDLERS
        if "SET LOCAL jit = off" not in (ROUTES / name).read_text()
    ]
    assert offenders == [], (
        "these shared-relation route modules run without a transaction-local "
        f"JIT policy: {offenders}"
    )


def test_the_policy_is_transaction_local():
    """`SET LOCAL` must sit inside a transaction in each handler."""
    for name in SHARED_AGGREGATE_HANDLERS:
        text = (ROUTES / name).read_text()
        if "SET LOCAL jit = off" not in text:
            continue
        assert ".begin()" in text, f"{name}: SET LOCAL outside a transaction"
