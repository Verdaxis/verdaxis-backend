"""
Seed script to populate fictional Maersk vessels for buyer@buy.com account.
Run from the verdaxis-backend directory:
    python scripts/seed_maersk_vessels.py
"""
import asyncio
import sys
import os

# Add project root to path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from sqlalchemy import select, text
from app.seeds.safety import seed_session

MAERSK_VESSELS = [
    {
        "name": "Maersk Edmonton",
        "imo_number": "9919475",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 150000,
        "cii_rating": "A",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Compliant",
        "lat": 5.5,
        "lng": 72.0,
        "prev_lat": 4.0,
        "prev_lng": 68.0,
    },
    {
        "name": "Maersk Halifax",
        "imo_number": "9919487",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 148000,
        "cii_rating": "B",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Warning",
        "lat": 42.0,
        "lng": -32.0,
        "prev_lat": 44.0,
        "prev_lng": -28.0,
    },
    {
        "name": "Laura Maersk",
        "imo_number": "9919499",
        "vessel_type": "Methanol-Ready Container Ship",
        "flag_state": "DK",
        "dwt": 172000,
        "cii_rating": "A",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Compliant",
        "lat": 28.0,
        "lng": -155.0,
        "prev_lat": 30.0,
        "prev_lng": -150.0,
    },
    {
        "name": "Maersk Hangzhou",
        "imo_number": "9919504",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 145000,
        "cii_rating": "C",
        "eu_ets_status": "Warning",
        "fueleu_status": "Compliant",
        "lat": 35.0,
        "lng": -48.0,
        "prev_lat": 36.0,
        "prev_lng": -44.0,
    },
    {
        "name": "Maersk Cape Town",
        "imo_number": "9919516",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 155000,
        "cii_rating": "B",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Compliant",
        "lat": -15.0,
        "lng": 55.0,
        "prev_lat": -18.0,
        "prev_lng": 50.0,
    },
    {
        "name": "Maersk Kotka",
        "imo_number": "9919528",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 140000,
        "cii_rating": "A",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Compliant",
        "lat": 53.55,
        "lng": 9.99,
        "prev_lat": 53.55,
        "prev_lng": 9.99,
    },
    {
        "name": "Maersk Guayaquil",
        "imo_number": "9919530",
        "vessel_type": "Container Ship",
        "flag_state": "DK",
        "dwt": 152000,
        "cii_rating": "B",
        "eu_ets_status": "Compliant",
        "fueleu_status": "Warning",
        "lat": -5.0,
        "lng": -52.0,
        "prev_lat": -2.0,
        "prev_lng": -56.0,
    },
    {
        "name": "Maersk Saltoro",
        "imo_number": "9919542",
        "vessel_type": "Tanker",
        "flag_state": "DK",
        "dwt": 110000,
        "cii_rating": "D",
        "eu_ets_status": "Warning",
        "fueleu_status": "Non-Compliant",
        "lat": 23.5,
        "lng": 62.0,
        "prev_lat": 24.0,
        "prev_lng": 58.0,
    },
]


async def seed_vessels():
    async with seed_session() as session:
        # 1. Find the buyer@buy.com user and their organization
        result = await session.execute(
            text("SELECT id, organization_id FROM users WHERE email = :email"),
            {"email": "buyer@buy.com"},
        )
        user_row = result.first()

        if not user_row:
            print("ERROR: User buyer@buy.com not found in database.")
            print("Make sure the user exists first.")
            return

        org_id = user_row.organization_id
        if not org_id:
            print("ERROR: buyer@buy.com has no organization_id.")
            print("The user needs to be assigned to an organization first.")
            return

        print(f"Found buyer@buy.com with org_id: {org_id}")

        # 2. Delete existing demo vessels for this org (idempotent)
        imo_numbers = [v["imo_number"] for v in MAERSK_VESSELS]
        placeholders = ", ".join(f":imo_{i}" for i in range(len(imo_numbers)))
        params = {f"imo_{i}": imo for i, imo in enumerate(imo_numbers)}

        await session.execute(
            text(f"DELETE FROM vessels WHERE imo_number IN ({placeholders})"),
            params,
        )
        print("Cleared any existing demo vessels.")

        # 3. Insert new vessels
        for v in MAERSK_VESSELS:
            location_wkt = f"POINT({v['lng']} {v['lat']})"
            prev_location_wkt = f"POINT({v['prev_lng']} {v['prev_lat']})"

            await session.execute(
                text("""
                    INSERT INTO vessels (
                        id, organization_id, name, imo_number, vessel_type, flag_state, dwt,
                        cii_rating, eu_ets_status, fueleu_status,
                        current_location, previous_location, updated_at
                    ) VALUES (
                        gen_random_uuid(), :org_id, :name, :imo_number, :vessel_type, :flag_state, :dwt,
                        :cii_rating, :eu_ets_status, :fueleu_status,
                        ST_GeogFromText(:current_loc), ST_GeogFromText(:prev_loc), NOW()
                    )
                """),
                {
                    "org_id": org_id,
                    "name": v["name"],
                    "imo_number": v["imo_number"],
                    "vessel_type": v["vessel_type"],
                    "flag_state": v["flag_state"],
                    "dwt": v["dwt"],
                    "cii_rating": v["cii_rating"],
                    "eu_ets_status": v["eu_ets_status"],
                    "fueleu_status": v["fueleu_status"],
                    "current_loc": location_wkt,
                    "prev_loc": prev_location_wkt,
                },
            )
            print(f"  Inserted: {v['name']} (IMO: {v['imo_number']})")

        await session.commit()
        print(f"\nDone! Inserted {len(MAERSK_VESSELS)} Maersk vessels for org {org_id}.")


if __name__ == "__main__":
    asyncio.run(seed_vessels())
