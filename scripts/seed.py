import argparse
import asyncio
import logging

from app.seeds import seed_all
from app.seeds.safety import seed_session

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Seed Verdaxis catalog and market data")
    parser.add_argument(
        '--reset-market',
        action='store_true',
        help='Clear and reseed market data so the demo state is reset for a fresh recording run.',
    )
    return parser.parse_args()


async def main() -> None:
    args = parse_args()
    logger.info("Running catalog + market seeds...")
    async with seed_session() as db:
        await seed_all(db, force_reset_market=args.reset_market)
        await db.commit()
    logger.info("Seed complete.")


if __name__ == "__main__":
    asyncio.run(main())
