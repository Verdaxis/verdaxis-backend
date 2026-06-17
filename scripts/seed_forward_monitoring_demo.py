import asyncio
import logging
import sys
from pathlib import Path


BACKEND_DIR = Path(__file__).resolve().parents[1]
STAGING_BACKEND_DIR = Path("/home/verdaxis-prod/verdaxis/staging/be")


def assert_staging_runtime() -> None:
    if BACKEND_DIR != STAGING_BACKEND_DIR:
        raise SystemExit(
            "Refusing to seed Forward Curve demo monitoring data outside staging. "
            f"Expected {STAGING_BACKEND_DIR}, got {BACKEND_DIR}."
        )


assert_staging_runtime()
sys.path.insert(0, str(BACKEND_DIR))

from app.database import AsyncSessionLocal  # noqa: E402
from app.seeds.catalog_seed import seed_catalog  # noqa: E402
from app.seeds.forward_monitoring_seed import seed_forward_monitoring_demo_data  # noqa: E402


logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


async def main() -> None:
    logger.info("Running explicit Forward Curve monitoring demo seed...")
    async with AsyncSessionLocal() as db:
        await seed_catalog(db)
        result = await seed_forward_monitoring_demo_data(db, reset=True)
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
