"""Run one database-locked news refresh for a systemd timer invocation."""

import asyncio

from app.database import AsyncSessionLocal
from app.services.news_feed import NewsRefreshInProgress, refresh_news


async def run_once() -> int:
    async with AsyncSessionLocal() as db:
        try:
            inserted = await refresh_news(db)
            await db.commit()
            return inserted
        except NewsRefreshInProgress:
            await db.rollback()
            return 0


def main() -> int:
    inserted = asyncio.run(run_once())
    print(f"news refresh complete: inserted={inserted}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
