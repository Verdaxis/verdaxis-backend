"""Operator CLI for trusted market-signal ingestion.

Reads one CSV file (one signal family, one source) and ingests it through
``app.services.market_signal_ingestion``. DRY RUN by default; ``--commit``
required to write. Guarded to the staging tree and the staging database,
mirroring scripts/seed_forward_monitoring_demo.py.

CSV headers per family (exact):
  indications: source_event_id,market_product,delivery_point,availability_window,side,price_per_mt_usd,quantity_mt,observed_at
  fair_bands:  source_event_id,market_product,delivery_point,availability_window,low_price_per_mt_usd,mid_price_per_mt_usd,high_price_per_mt_usd,model_name,model_version,observed_at
  stems:       source_event_id,stem_uid,market_product,delivery_point,availability_window,status,quantity_mt,stem_start,stem_end,observed_at
"""
import argparse
import asyncio
import csv
import logging
import re
import sys
from pathlib import Path
from urllib.parse import urlparse


BACKEND_DIR = Path(__file__).resolve().parents[1]
STAGING_BACKEND_DIR = Path("/home/verdaxis-prod/verdaxis/staging/be")
STAGING_DATABASE_NAME = "verdaxis_staging"

CLI_FAMILIES = {
    "indications": "MARKET_INDICATION",
    "fair_bands": "FAIR_PRICE_BAND",
    "stems": "PHYSICAL_STEM",
}

# Stored, joined on, and rendered in reports: never free-text junk.
SOURCE_PATTERN = re.compile(r"^[a-z0-9][a-z0-9_-]{1,63}$")


def assert_staging_runtime() -> None:
    if BACKEND_DIR != STAGING_BACKEND_DIR:
        raise SystemExit(
            "Refusing to ingest market signals outside staging. "
            f"Expected {STAGING_BACKEND_DIR}, got {BACKEND_DIR}."
        )


assert_staging_runtime()
sys.path.insert(0, str(BACKEND_DIR))

from app.config import settings  # noqa: E402
from app.database import AsyncSessionLocal  # noqa: E402
from app.services.market_signal_ingestion import ingest_signals  # noqa: E402


logging.basicConfig(level=logging.INFO, format="%(message)s")
logger = logging.getLogger(__name__)


def assert_expected_database(expected_name: str) -> None:
    """Refuse to run against any database other than the expected one.

    ``--allow-db`` exists only to loosen the name check for a future,
    explicitly signed-off production decision; using it against prod requires
    Jon's explicit sign-off (see docs/market-signal-ingestion.md).
    """
    configured_name = settings.DATABASE_NAME
    url_path = urlparse(settings.DATABASE_URL or "").path.strip("/")
    if configured_name != expected_name or url_path != expected_name:
        raise SystemExit(
            "Refusing to ingest market signals into an unexpected database. "
            f"Expected database {expected_name!r}; got configured_name={configured_name!r}, "
            f"url_database={url_path!r}."
        )


def read_rows(file_path: Path) -> list[dict]:
    if not file_path.is_file():
        raise SystemExit(f"CSV file not found: {file_path}")
    with file_path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle)
        if reader.fieldnames is None:
            raise SystemExit(f"CSV file has no header row: {file_path}")
        if None in reader.fieldnames or "" in reader.fieldnames:
            raise SystemExit(f"CSV header contains blank column names: {file_path}")
        return list(reader)


def print_report(report) -> None:
    mode = "DRY RUN (no writes)" if report.dry_run else "COMMIT"
    logger.info("── Ingest report ─────────────────────────────")
    logger.info("mode:               %s", mode)
    logger.info("family:             %s", report.family)
    logger.info("source:             %s", report.source)
    logger.info("run id:             %s", report.created_run_id or "-")
    logger.info("inserted:           %d", report.inserted)
    if report.dry_run:
        logger.info("would insert:       %d", report.would_insert)
    logger.info("skipped duplicates: %d", report.skipped_duplicates)
    logger.info("stale warnings:     %d (imported but older than the board lookback)", report.stale_warnings)
    logger.info("errors:             %d", len(report.errors))
    for error in report.errors:
        logger.info("  row %d: %s", error.row_number, error.reason)
    logger.info("──────────────────────────────────────────────")


async def run(args: argparse.Namespace) -> int:
    rows = read_rows(Path(args.file))
    family = CLI_FAMILIES[args.family]
    dry_run = not args.commit

    async with AsyncSessionLocal() as db:
        report = await ingest_signals(
            db, family=family, source=args.source, rows=rows, dry_run=dry_run
        )
        if not dry_run and report.inserted > 0:
            await db.commit()
        else:
            await db.rollback()

    print_report(report)
    if not dry_run and report.inserted == 0:
        logger.info("Nothing inserted; no run was written.")
    return 1 if report.errors and report.inserted == 0 else 0


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--family", required=True, choices=sorted(CLI_FAMILIES))
    parser.add_argument("--source", required=True, help="run source id, e.g. desk-marks or broker-sheet-a")
    parser.add_argument("--file", required=True, help="path to the CSV file for this run")
    parser.add_argument("--commit", action="store_true", help="actually write (default is dry run)")
    parser.add_argument(
        "--allow-db",
        default=STAGING_DATABASE_NAME,
        help="expected database name (default verdaxis_staging); overriding toward prod requires explicit sign-off",
    )
    args = parser.parse_args()

    if not SOURCE_PATTERN.fullmatch(args.source):
        raise SystemExit(
            f"--source must match {SOURCE_PATTERN.pattern} (lowercase, digits, '-', '_'), got {args.source!r}"
        )
    assert_expected_database(args.allow_db)

    exit_code = asyncio.run(run(args))
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()
