"""Seed entry point — call seed_all() to populate catalog + market data."""

from sqlalchemy.ext.asyncio import AsyncSession

from app.seeds.catalog_seed import seed_catalog
from app.seeds.market_seed import seed_market_data


async def seed_all(
    db: AsyncSession,
    *,
    force_reset_market: bool = False,
    allow_demo_reset: bool = False,
) -> None:
    """Run all seed functions in dependency order.  Each is idempotent unless force_reset_market is set."""
    await seed_catalog(db)
    await seed_market_data(db, force_reset=force_reset_market, allow_demo_reset=allow_demo_reset)
