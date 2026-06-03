#!/usr/bin/env python3
"""Run one disclosed demo market activity tick."""

from __future__ import annotations

import asyncio
import json

from app.database import AsyncSessionLocal
from app.services.demo_activity import generate_demo_market_activity


async def main() -> None:
    async with AsyncSessionLocal() as db:
        result = await generate_demo_market_activity(db)
    print(json.dumps(result, default=str, sort_keys=True))


if __name__ == "__main__":
    asyncio.run(main())
