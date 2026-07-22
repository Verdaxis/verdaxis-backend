import asyncio
import logging
import sys
from pathlib import Path


BACKEND_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BACKEND_DIR))

from sqlalchemy import select  # noqa: E402

from app.models.catalog import DeliveryPoint  # noqa: E402
from app.seeds.catalog_seed import DELIVERY_POINT_IDS  # noqa: E402
from app.seeds.forward_monitoring_seed import seed_forward_monitoring_demo_data  # noqa: E402
from app.seeds.safety import seed_session  # noqa: E402


logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def assert_catalog_ready(db) -> None:
    expected_ids = set(DELIVERY_POINT_IDS.values())
    result = await db.execute(
        select(DeliveryPoint.id).where(
            DeliveryPoint.id.in_(expected_ids),
            DeliveryPoint.is_active.is_(True),
        )
    )
    found_ids = set(result.scalars().all())
    missing = expected_ids - found_ids
    if missing:
        raise SystemExit(
            "Refusing to seed Forward Curve demo monitoring data because staging catalog delivery points "
            f"are missing or inactive: {len(missing)} missing."
        )


async def main() -> None:
    logger.info("Running explicit Forward Curve monitoring demo seed...")
    async with seed_session() as db:
        await assert_catalog_ready(db)
        result = await seed_forward_monitoring_demo_data(db)
    logger.info(
        "Forward monitoring demo seed complete. "
        "deleted=(indications:%s fair_bands:%s stems:%s) "
        "inserted=(indications:%s fair_bands:%s stems:%s)",
        result.deleted_indications,
        result.deleted_fair_price_bands,
        result.deleted_physical_stems,
        result.inserted_indications,
        result.inserted_fair_price_bands,
        result.inserted_physical_stems,
    )


if __name__ == "__main__":
    asyncio.run(main())
