"""Seed entry point — call seed_all() to populate catalog + market data."""

from sqlalchemy.ext.asyncio import AsyncSession

from app.seeds.catalog_seed import seed_catalog
from app.seeds.market_seed import seed_market_data


async def seed_all(db: AsyncSession) -> None:
    """Run all seed functions in dependency order.  Each is idempotent."""
    await seed_catalog(db)
    await seed_market_data(db)
