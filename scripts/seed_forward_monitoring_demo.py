import asyncio
import logging
import sys
from pathlib import Path
from urllib.parse import urlparse


BACKEND_DIR = Path(__file__).resolve().parents[1]
STAGING_BACKEND_DIR = Path("/home/verdaxis-prod/verdaxis/staging/be")
STAGING_DATABASE_NAME = "verdaxis_staging"


def assert_staging_runtime() -> None:
    if BACKEND_DIR != STAGING_BACKEND_DIR:
        raise SystemExit(
            "Refusing to seed Forward Curve demo monitoring data outside staging. "
            f"Expected {STAGING_BACKEND_DIR}, got {BACKEND_DIR}."
        )


assert_staging_runtime()
sys.path.insert(0, str(BACKEND_DIR))

from sqlalchemy import select  # noqa: E402

from app.config import settings  # noqa: E402
from app.database import AsyncSessionLocal  # noqa: E402
from app.models.catalog import DeliveryPoint  # noqa: E402
from app.seeds.catalog_seed import DELIVERY_POINT_IDS  # noqa: E402
from app.seeds.forward_monitoring_seed import seed_forward_monitoring_demo_data  # noqa: E402


logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


def assert_staging_database() -> None:
    configured_name = settings.DATABASE_NAME
    url_path = urlparse(settings.DATABASE_URL or "").path.strip("/")
    if configured_name != STAGING_DATABASE_NAME or url_path != STAGING_DATABASE_NAME:
        raise SystemExit(
            "Refusing to seed Forward Curve demo monitoring data outside the staging database. "
            f"Expected database {STAGING_DATABASE_NAME!r}; got configured_name={configured_name!r}, "
            f"url_database={url_path!r}."
        )


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
    assert_staging_database()
    logger.info("Running explicit Forward Curve monitoring demo seed...")
    async with AsyncSessionLocal() as db:
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
