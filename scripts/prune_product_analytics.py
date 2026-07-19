#!/usr/bin/env python3
"""Idempotent daily prune for the ``user_login_days`` fact table.

Retains 800 UTC calendar dates — today plus the previous 799 — so a maximum
365-day analytics range and its equivalent previous period always remain
available with buffer (plan §2.4). Rows with
``activity_date < current_utc_date - 799`` are deleted.

``user_status_transitions`` is durable business history and is NEVER pruned.

Run by the verdaxis-product-analytics-prune systemd timer (see
deploy/systemd/); a nonzero exit leaves the oneshot unit failed for the
existing monitor to alert on.
"""

from __future__ import annotations

import asyncio
import sys
from datetime import UTC, date, datetime, timedelta
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from sqlalchemy import delete  # noqa: E402
from sqlalchemy.ext.asyncio import async_sessionmaker, create_async_engine  # noqa: E402

RETAINED_DATES = 800


def compute_cutoff(today: date) -> date:
    """First retained date is ``today - 799``; anything older is deleted."""
    return today - timedelta(days=RETAINED_DATES - 1)


async def prune_login_days(session, *, today: date | None = None) -> int:
    """Delete login-day rows older than the retention window. Idempotent."""
    from app.models.product_analytics import UserLoginDay

    cutoff = compute_cutoff(today or datetime.now(UTC).date())
    result = await session.execute(
        delete(UserLoginDay).where(UserLoginDay.activity_date < cutoff)
    )
    await session.commit()
    return result.rowcount or 0


async def main() -> int:
    from app.config import settings

    engine = create_async_engine(settings.DATABASE_URL, hide_parameters=True)
    try:
        factory = async_sessionmaker(engine)
        async with factory() as session:
            deleted = await prune_login_days(session)
        print(f"pruned {deleted} login-day rows older than {RETAINED_DATES} dates")
        return 0
    finally:
        await engine.dispose()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
