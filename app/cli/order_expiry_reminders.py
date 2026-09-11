"""Run one bounded order-expiry reminder transaction."""

import asyncio

from app.database import AsyncSessionLocal, verify_database_runtime
from app.services.order_expiry_reminders import remind_expiring_orders


async def run_once() -> int:
    await verify_database_runtime()
    async with AsyncSessionLocal() as db:
        sent = await remind_expiring_orders(db)
        await db.commit()
        return sent


if __name__ == "__main__":
    print(f"Order expiry reminders created: {asyncio.run(run_once())}")
