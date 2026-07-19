#!/usr/bin/env python3
"""
Seed script to populate the vessels table with 65 realistic maritime vessels.
Distributed across 7 buyer shipping organizations.

This script is idempotent — it deletes existing demo vessels (IMO 98*) before inserting.

Execution:
    cd /home/verdaxis-prod/verdaxis-backend
    python scripts/seed_vessels_fleet.py

NOTE: Run with verdaxis-prod user permissions for database connectivity.
File ownership: sudo chown verdaxis-prod:verdaxis-prod /path/to/script
"""

import psycopg2
import psycopg2.extras
import random
import uuid
from datetime import datetime
from app.seeds.safety import seed_connection


# Database connection
def get_db_connection():
    """Establish connection to verdaxis database."""
    conn = seed_connection()
    conn.autocommit = False
    return conn


# Organization IDs (buyer shipping lines)
ORGANIZATION_IDS = {
    "Buy Corp": "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "Maersk Fuel Procurement": "4da7b285-34ee-5443-9406-f96b4ed1a251",
    "Evergreen Marine Bunkers": "0dbce576-2026-5925-ab66-674d505e98ad",
    "COSCO Energy Trading": "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a",
    "MSC Fuel Desk": "277491df-cb0d-5f2d-a2cf-5746829c6da6",
    "CMA CGM Green Fuel": "3b302066-d65c-5c3e-8fcc-70b3da3bcafd",
    "MarinaChain": "97d4727d-fe17-4be8-bd2b-66653a4f8b76",
}

# Distribution: which org gets how many vessels
DISTRIBUTION = {
    "Maersk Fuel Procurement": 12,
    "MSC Fuel Desk": 10,
    "CMA CGM Green Fuel": 10,
    "COSCO Energy Trading": 10,
    "Evergreen Marine Bunkers": 8,
    "Buy Corp": 8,
    "MarinaChain": 7,
}

# Vessel naming conventions per company
NAMING_CONVENTIONS = {
    "Maersk Fuel Procurement": [
        "Maersk Eindhoven", "Maersk Skarstind", "Maersk Tanjong", "Maersk Honam",
        "Maersk Sealand", "Maersk Saigon", "Maersk Brenntag", "Maersk Triangulation",
        "Maersk Kendal", "Maersk Kingston", "Maersk Carolina", "Maersk Kalmar",
    ],
    "MSC Fuel Desk": [
        "MSC Oscar", "MSC Gülsün", "MSC Maya", "MSC Irina",
        "MSC Thalassa", "MSC Luna", "MSC Sola", "MSC Miriam",
        "MSC Artemisia", "MSC Evora",
    ],
    "CMA CGM Green Fuel": [
        "CMA CGM Jacques Saadé", "CMA CGM Antoine", "CMA CGM Chixoy",
        "CMA CGM Djakarta", "CMA CGM Ama", "CMA CGM Balzac",
        "CMA CGM Centaure", "CMA CGM Cristobal", "CMA CGM Concorde",
        "CMA CGM Andromeda",
    ],
    "COSCO Energy Trading": [
        "COSCO Shipping Universe", "COSCO Shipping Integrity", "COSCO Shipping Victory",
        "COSCO Shipping Aries", "COSCO Shipping Libra", "COSCO Shipping Taurus",
        "COSCO Shipping Gemini", "COSCO Shipping Aquarius", "COSCO Shipping Pisces",
        "COSCO Shipping Virgo",
    ],
    "Evergreen Marine Bunkers": [
        "Ever Given", "Ever Ace", "Ever Alp", "Ever Alert",
        "Ever Ardent", "Ever Asia", "Ever Assured", "Ever Apex",
    ],
    "Buy Corp": [
        "Pacific Dawn", "Pacific Pride", "Pacific Pioneer", "Pacific Power",
        "Atlantic Spirit", "Atlantic Star", "Atlantic Strength", "Atlantic Swift",
    ],
    "MarinaChain": [
        "MC Endeavor", "MC Enterprise", "MC Explorer", "MC Excellence",
        "MC Excelsior", "MC Express", "MC Exceed", "MC Exemplar",
    ],
}

# Major shipping routes with coordinates (for realistic current positions)
SHIPPING_ROUTES = [
    # Malacca Strait
    {"name": "Malacca Strait", "center": (101.0, 2.5), "radius": 1.5},
    # Singapore Strait
    {"name": "Singapore Strait", "center": (104.0, 1.2), "radius": 1.0},
    # South China Sea
    {"name": "South China Sea", "center": (113.0, 12.0), "radius": 3.0},
    # Suez Canal approaches
    {"name": "Suez Canal", "center": (32.5, 30.5), "radius": 2.0},
    # Mediterranean
    {"name": "Mediterranean", "center": (16.0, 36.0), "radius": 5.0},
    # English Channel
    {"name": "English Channel", "center": (0.5, 50.5), "radius": 1.5},
    # Gulf of Aden
    {"name": "Gulf of Aden", "center": (47.0, 13.0), "radius": 2.0},
    # Arabian Sea
    {"name": "Arabian Sea", "center": (65.0, 18.0), "radius": 4.0},
    # North Atlantic
    {"name": "North Atlantic", "center": (-20.0, 40.0), "radius": 8.0},
    # Gulf of Mexico
    {"name": "Gulf of Mexico", "center": (-87.0, 26.0), "radius": 3.0},
    # North Pacific
    {"name": "North Pacific", "center": (155.0, 35.0), "radius": 8.0},
    # Indian Ocean
    {"name": "Indian Ocean", "center": (77.0, 2.0), "radius": 5.0},
    # Panama Canal
    {"name": "Panama Canal", "center": (-79.5, 9.0), "radius": 1.0},
    # Cape of Good Hope
    {"name": "Cape of Good Hope", "center": (19.0, -33.5), "radius": 1.5},
    # Bay of Bengal
    {"name": "Bay of Bengal", "center": (88.0, 14.0), "radius": 3.0},
    # East China Sea
    {"name": "East China Sea", "center": (126.0, 29.0), "radius": 3.0},
]


def generate_vessel_data():
    """
    Generate realistic vessel data distributed across organizations.
    Returns list of vessel dictionaries.
    """
    vessels = []
    imo_counter = 9800001
    vessel_id = 0

    for org_name, count in DISTRIBUTION.items():
        org_id = ORGANIZATION_IDS[org_name]
        names = NAMING_CONVENTIONS[org_name]

        for i in range(count):
            vessel_id += 1

            # Cycle through names
            name = names[i % len(names)]
            if i >= len(names):
                name = f"{name} {i // len(names)}"

            # Generate IMO number
            imo_number = str(imo_counter)
            imo_counter += 1

            # Vessel type distribution: Container (50%), Tanker (20%), Bulk (15%), LNG (10%), Car (5%)
            vessel_type_rand = random.random()
            if vessel_type_rand < 0.50:
                vessel_type = "Container Ship"
                dwt = random.randint(40000, 220000)
            elif vessel_type_rand < 0.70:
                vessel_type = "Tanker"
                dwt = random.randint(50000, 320000)
            elif vessel_type_rand < 0.85:
                vessel_type = "Bulk Carrier"
                dwt = random.randint(60000, 210000)
            elif vessel_type_rand < 0.95:
                vessel_type = "LNG Carrier"
                dwt = random.randint(70000, 100000)
            else:
                vessel_type = "Car Carrier"
                dwt = random.randint(15000, 25000)

            # Flag state mix
            flag_states = ["PA", "MH", "LR", "HK", "SG", "BS", "MT", "GR"]
            flag_state = random.choice(flag_states)

            # CII rating distribution: A (15%), B (40%), C (30%), D (15%)
            cii_rand = random.random()
            if cii_rand < 0.15:
                cii_rating = "A"
            elif cii_rand < 0.55:
                cii_rating = "B"
            elif cii_rand < 0.85:
                cii_rating = "C"
            else:
                cii_rating = "D"

            # EU ETS status: Compliant (70%), Phase-In (20%), Non-Compliant (10%)
            ets_rand = random.random()
            if ets_rand < 0.70:
                eu_ets_status = "Compliant"
            elif ets_rand < 0.90:
                eu_ets_status = "Phase-In"
            else:
                eu_ets_status = "Non-Compliant"

            # FuelEU status: Compliant (60%), Monitoring (30%), Non-Compliant (10%)
            fueleu_rand = random.random()
            if fueleu_rand < 0.60:
                fueleu_status = "Compliant"
            elif fueleu_rand < 0.90:
                fueleu_status = "Monitoring"
            else:
                fueleu_status = "Non-Compliant"

            # Current location: random on a shipping route
            route = random.choice(SHIPPING_ROUTES)
            center_lng, center_lat = route["center"]
            lng_offset = (random.random() - 0.5) * route["radius"] * 2
            lat_offset = (random.random() - 0.5) * route["radius"] * 2
            current_lng = center_lng + lng_offset
            current_lat = center_lat + lat_offset

            # Previous location: 1-3 degrees away (simulating 1-3 day transit)
            prev_lng = current_lng + random.uniform(-3, 3)
            prev_lat = current_lat + random.uniform(-3, 3)

            vessel = {
                "id": str(uuid.uuid4()),
                "organization_id": org_id,
                "name": name,
                "imo_number": imo_number,
                "vessel_type": vessel_type,
                "flag_state": flag_state,
                "dwt": dwt,
                "cii_rating": cii_rating,
                "eu_ets_status": eu_ets_status,
                "fueleu_status": fueleu_status,
                "current_lng": current_lng,
                "current_lat": current_lat,
                "prev_lng": prev_lng,
                "prev_lat": prev_lat,
            }
            vessels.append(vessel)

    return vessels


def format_geography_point(lng, lat):
    """
    Format geography point for PostgreSQL.
    Returns SRID=4326;POINT(lng lat) format.
    """
    return f"SRID=4326;POINT({lng} {lat})"


def insert_vessels(conn, vessels):
    """
    Insert vessels into database.
    Returns count of inserted vessels.
    """
    cur = conn.cursor()

    try:
        # 1. Delete existing demo vessels (idempotent)
        print("Deleting existing demo vessels (IMO starting with 98)...")
        cur.execute("DELETE FROM vessels WHERE imo_number LIKE '98%'")
        deleted_count = cur.rowcount
        print(f"  Deleted {deleted_count} existing demo vessels")

        # 2. Insert new vessels
        print("\nInserting new vessels...")
        inserted = 0

        for vessel in vessels:
            current_location = format_geography_point(vessel["current_lng"], vessel["current_lat"])
            previous_location = format_geography_point(vessel["prev_lng"], vessel["prev_lat"])

            cur.execute("""
                INSERT INTO vessels (
                    id, organization_id, name, imo_number, vessel_type,
                    flag_state, dwt, cii_rating, eu_ets_status, fueleu_status,
                    current_location, previous_location, updated_at
                ) VALUES (
                    %s, %s, %s, %s, %s,
                    %s, %s, %s, %s, %s,
                    ST_GeogFromText(%s), ST_GeogFromText(%s), NOW()
                )
            """, (
                vessel["id"],
                vessel["organization_id"],
                vessel["name"],
                vessel["imo_number"],
                vessel["vessel_type"],
                vessel["flag_state"],
                vessel["dwt"],
                vessel["cii_rating"],
                vessel["eu_ets_status"],
                vessel["fueleu_status"],
                current_location,
                previous_location,
            ))
            inserted += 1

            if inserted % 10 == 0:
                print(f"  Inserted {inserted}/{len(vessels)} vessels...")

        # Commit transaction
        conn.commit()
        print(f"\nSuccessfully committed {inserted} vessels to database.")
        return inserted

    except Exception as e:
        conn.rollback()
        print(f"ERROR during insert: {e}")
        raise
    finally:
        cur.close()


def main():
    """Main entry point."""
    print("=" * 70)
    print("Verdaxis Maritime Vessels Fleet Seed Script")
    print("=" * 70)

    # Generate vessel data
    print("\nGenerating vessel data...")
    vessels = generate_vessel_data()
    print(f"Generated {len(vessels)} realistic vessels across {len(DISTRIBUTION)} organizations:")
    for org_name, count in DISTRIBUTION.items():
        print(f"  - {org_name}: {count} vessels")

    # Connect to database
    print("\nConnecting to database...")
    try:
        conn = get_db_connection()
        print("Connected successfully.")
    except Exception as e:
        print(f"ERROR: Failed to connect to database: {e}")
        return 1

    # Insert vessels
    try:
        inserted = insert_vessels(conn, vessels)
        print("\n" + "=" * 70)
        print(f"SUCCESS: Inserted {inserted} vessels into vessels table")
        print("=" * 70)
        return 0
    except Exception as e:
        print(f"\nFAILED: {e}")
        return 1
    finally:
        conn.close()


if __name__ == "__main__":
    exit(main())
