"""Run one bounded authentication-maintenance transaction."""

import argparse
import asyncio

from app.database import AsyncSessionLocal
from app.services.account_approval_email import (
    DEFAULT_APPROVAL_EMAIL_BATCH_SIZE,
    retry_pending_account_approval_emails,
)
from app.services.auth_maintenance import DEFAULT_BATCH_SIZE, run_auth_maintenance


async def run_once(
    *,
    batch_size: int = DEFAULT_BATCH_SIZE,
    email_batch_size: int = DEFAULT_APPROVAL_EMAIL_BATCH_SIZE,
) -> dict[str, int]:
    async with AsyncSessionLocal() as db:
        report = await run_auth_maintenance(db, batch_size=batch_size)
        await db.commit()
        report.update(
            await retry_pending_account_approval_emails(
                db,
                batch_size=email_batch_size,
            )
        )
        return report


def main() -> int:
    parser = argparse.ArgumentParser(description="Bounded Verdaxis auth-state maintenance")
    parser.add_argument("--batch-size", type=int, default=DEFAULT_BATCH_SIZE)
    parser.add_argument(
        "--email-batch-size",
        type=int,
        default=DEFAULT_APPROVAL_EMAIL_BATCH_SIZE,
    )
    args = parser.parse_args()
    report = asyncio.run(
        run_once(
            batch_size=args.batch_size,
            email_batch_size=args.email_batch_size,
        )
    )
    print("auth maintenance complete: " + " ".join(f"{key}={value}" for key, value in sorted(report.items())))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
