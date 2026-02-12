"""
Import GENA CSV data into the producer_projects table.

Usage:
    python -m scripts.import_gena_csv --file data/gena_methanol_projects.csv --fuel-type Methanol
    python -m scripts.import_gena_csv --file data/gena_ethanol_projects.csv --fuel-type Ethanol

Expected CSV columns:
    project_name, country, region, capacity_kt, cod_year, status, feedstock, technology, lat, lng, gena_id
"""
import asyncio
import argparse
import csv
import sys
from decimal import Decimal
from pathlib import Path

# Add project root to path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from sqlalchemy import select
from app.database import AsyncSessionLocal
from app.models.producer import ProducerProject, ProjectStatus
from geoalchemy2.elements import WKTElement


STATUS_MAP = {
    "announced": ProjectStatus.ANNOUNCED,
    "under construction": ProjectStatus.UNDER_CONSTRUCTION,
    "operational": ProjectStatus.OPERATIONAL,
    "cancelled": ProjectStatus.CANCELLED,
}


async def import_csv(file_path: str, fuel_type: str):
    async with AsyncSessionLocal() as db:
        with open(file_path, 'r') as f:
            reader = csv.DictReader(f)
            created = 0
            skipped = 0

            for row in reader:
                gena_id = row.get('gena_id', '').strip()

                # Skip if already imported (idempotent)
                if gena_id:
                    existing = await db.execute(
                        select(ProducerProject).where(ProducerProject.gena_project_id == gena_id)
                    )
                    if existing.scalar_one_or_none():
                        skipped += 1
                        continue

                lat = float(row['lat']) if row.get('lat') else None
                lng = float(row['lng']) if row.get('lng') else None
                location = WKTElement(f'POINT({lng} {lat})', srid=4326) if lat and lng else None

                project = ProducerProject(
                    name=row['project_name'].strip(),
                    fuel_type=fuel_type,
                    capacity_kt_per_year=Decimal(row['capacity_kt']) if row.get('capacity_kt') else None,
                    country=row['country'].strip(),
                    region=row.get('region', '').strip() or None,
                    location=location,
                    cod_year=int(row['cod_year']) if row.get('cod_year') else None,
                    status=STATUS_MAP.get(row.get('status', '').lower().strip(), ProjectStatus.ANNOUNCED),
                    data_source="GENA",
                    gena_project_id=gena_id or None,
                    feedstock=row.get('feedstock', '').strip() or None,
                    technology=row.get('technology', '').strip() or None,
                )
                db.add(project)
                created += 1

            await db.commit()
            print(f"Import complete: {created} created, {skipped} skipped (already exist)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Import GENA CSV into producer_projects")
    parser.add_argument("--file", required=True, help="Path to CSV file")
    parser.add_argument("--fuel-type", required=True, help="Fuel type (Methanol, Ethanol, etc.)")
    args = parser.parse_args()

    asyncio.run(import_csv(args.file, args.fuel_type))
