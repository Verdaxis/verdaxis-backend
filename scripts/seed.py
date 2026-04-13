import asyncio
import logging

from app.database import AsyncSessionLocal
from app.seeds import seed_all

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def main() -> None:
    logger.info("Running catalog + market seeds...")
    async with AsyncSessionLocal() as db:
        await seed_all(db)
        await db.commit()
    logger.info("Seed complete.")


if __name__ == "__main__":
    asyncio.run(main())
