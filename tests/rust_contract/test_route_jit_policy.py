"""The paged orderbook handlers keep JIT policy transaction-local.

The expanded corpus makes the shared eligibility subplans cross PostgreSQL's JIT
cost threshold, so a page that returns a few rows can still pay hundreds of
milliseconds of JIT compilation. The orderbook lists and orderbook map handlers
begin a transaction and `SET LOCAL jit = off` before their shared-relation
query; the listings handler (R27) did not, which produced the bimodal ~0.9 s
tail the Task 14a measurement found.

This test scans the three paged orderbook route sources so a new handler cannot
silently omit the same policy. It asserts a source property, not a runtime one;
the corpus and the latency probe still own the behaviour.
"""

from pathlib import Path

ROUTES = Path(__file__).resolve().parents[2] / "rs" / "read" / "src" / "routes"

# The three routes that page the expanded shared eligible-order relation.
PAGED_ORDERBOOK_HANDLERS = ("orderbook_lists.rs", "orderbook_map.rs", "listings.rs")


def test_paged_orderbook_handlers_disable_jit_locally():
    offenders = [
        name
        for name in PAGED_ORDERBOOK_HANDLERS
        if "SET LOCAL jit = off" not in (ROUTES / name).read_text()
    ]
    assert offenders == [], (
        "these paged orderbook route modules run the shared relation without a "
        f"transaction-local JIT policy: {offenders}"
    )


def test_listings_specifically_disables_jit():
    text = (ROUTES / "listings.rs").read_text()
    assert "SET LOCAL jit = off" in text
    # The policy must sit inside a transaction so it cannot leak through the pool.
    assert "SET LOCAL" in text
    assert ".begin()" in text
