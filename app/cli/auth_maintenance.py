"""Run one bounded authentication-maintenance transaction."""

import argparse
import asyncio

from app.database import AsyncSessionLocal
from app.services.auth_maintenance import DEFAULT_BATCH_SIZE, run_auth_maintenance


async def run_once(*, batch_size: int = DEFAULT_BATCH_SIZE) -> dict[str, int]:
    async with AsyncSessionLocal() as db:
        report = await run_auth_maintenance(db, batch_size=batch_size)
        await db.commit()
        return report


def main() -> int:
    parser = argparse.ArgumentParser(description="Bounded Verdaxis auth-state maintenance")
    parser.add_argument("--batch-size", type=int, default=DEFAULT_BATCH_SIZE)
    args = parser.parse_args()
    report = asyncio.run(run_once(batch_size=args.batch_size))
    print("auth maintenance complete: " + " ".join(f"{key}={value}" for key, value in sorted(report.items())))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
