#!/usr/bin/env python3
"""Run one disclosed demo market activity tick."""

from __future__ import annotations

import asyncio
import json

from app.database import AsyncSessionLocal
from app.demo_identities import DEMO_ACTIVITY_ORG_IDS
from app.services.market_events import enqueue_market_events, participant_market_event
from app.services.demo_activity import (
    ensure_demo_activity_organizations,
    ensure_demo_market_coverage,
    generate_demo_market_activity,
    prune_demo_activity,
)


async def _enqueue_demo_refresh(db, *, operation: str, changed: bool) -> None:
    if not changed:
        return
    await enqueue_market_events(
        db,
        [
            participant_market_event(
                event_type="demo_market_changed",
                aggregate_type="demo_market",
                aggregate_id=operation,
                participant_org_ids=DEMO_ACTIVITY_ORG_IDS,
                payload={"operation": operation},
                public_market_invalidation=True,
            )
        ],
    )


def _validate_activity_result(result: dict[str, object]) -> None:
    """Accept a new pair or the service's proven idempotent tick result."""
    created_orders = result.get("created_orders")
    created_trades = result.get("created_trades")
    counts_are_integers = (
        type(created_orders) is int
        and type(created_trades) is int
    )

    if counts_are_integers and (created_orders, created_trades) == (2, 1):
        return
    if (
        counts_are_integers
        and (created_orders, created_trades) == (0, 0)
        and result.get("reason") == "already generated for tick"
    ):
        return
    raise RuntimeError("demo activity tick returned an unexpected result")


async def main() -> None:
    async with AsyncSessionLocal() as db:
        await ensure_demo_activity_organizations(db)
        await db.commit()
        coverage = await ensure_demo_market_coverage(db)
        await _enqueue_demo_refresh(
            db,
            operation="coverage",
            changed=any(
                coverage.get(key, 0)
                for key in (
                    "coverage_created",
                    "coverage_refreshed",
                    "coverage_expired",
                    "legacy_activity_expired",
                )
            ),
        )
        await db.commit()
        prune_result = await prune_demo_activity(db)
        await _enqueue_demo_refresh(
            db,
            operation="prune",
            changed=(
                isinstance(prune_result, dict)
                and any(prune_result.get(key, 0) for key in ("trades_pruned", "orders_pruned"))
            ),
        )
        await db.commit()
        result = await generate_demo_market_activity(db)
        _validate_activity_result(result)
        await _enqueue_demo_refresh(
            db,
            operation="tick",
            changed=bool(result.get("created_orders") or result.get("created_trades")),
        )
        await db.commit()
    print(json.dumps({**coverage, **result}, default=str, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
