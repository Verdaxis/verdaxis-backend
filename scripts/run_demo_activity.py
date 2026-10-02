#!/usr/bin/env python3
"""Run one disclosed demo market activity tick."""

from __future__ import annotations

import asyncio
import json

from app.database import AsyncSessionLocal
from app.services.demo_activity import (
    ensure_demo_activity_organizations,
    ensure_demo_market_coverage,
    generate_demo_market_activity,
    prune_demo_activity,
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
        await db.commit()
        await prune_demo_activity(db)
        await db.commit()
        result = await generate_demo_market_activity(db)
        _validate_activity_result(result)
        await db.commit()
    print(json.dumps({**coverage, **result}, default=str, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
