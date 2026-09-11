"""Queue due expiry reminders and deliver a bounded email batch."""

import asyncio

from app.database import AsyncSessionLocal, verify_database_runtime
from app.services.order_expiry_reminders import (
    remind_expiring_orders,
    deliver_expiry_reminder_emails,
)


async def run_once() -> dict[str, int]:
    await verify_database_runtime()
    async with AsyncSessionLocal() as db:
        created = await remind_expiring_orders(db)
        await db.commit()
        emails_sent = await deliver_expiry_reminder_emails(db)
        return {"reminders_created": created, "emails_sent": emails_sent}


if __name__ == "__main__":
    print(f"Order expiry reminders: {asyncio.run(run_once())}")
