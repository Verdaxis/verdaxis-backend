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


async def main() -> None:
    async with AsyncSessionLocal() as db:
        await ensure_demo_activity_organizations(db)
        await db.commit()
        coverage = await ensure_demo_market_coverage(db)
        await db.commit()
        await prune_demo_activity(db)
        await db.commit()
        result = await generate_demo_market_activity(db)
        await db.commit()
    print(json.dumps({**coverage, **result}, default=str, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
