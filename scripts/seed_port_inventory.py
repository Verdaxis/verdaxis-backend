#!/usr/bin/env python3
"""
Seed script for Verdaxis port inventory and port intelligence.
Populates inventory_items and ensures all ports have port_intelligence rows.
Idempotent: clears existing data and rebuilds from scratch.
"""

import uuid
import random
from datetime import datetime, timedelta
from decimal import Decimal
import psycopg2
from psycopg2.extras import execute_batch

# Database connection
DB_CONFIG = {
    "host": "localhost",
    "port": 5432,
    "database": "verdaxis",
    "user": "postgres",
    "password": "Tealtent477"
}

# Supplier organization IDs
SUPPLIERS = {
    "Vitol": "79609f48-0a3e-560e-a1e1-63d90601d84a",
    "Trafigura": "2c4e387e-de22-5adb-ad88-9274ba84ebe1",
    "Peninsula": "93ccda09-54b3-53ee-afc0-759d3048161f",
    "Bunker Holding": "82426590-0963-5486-9b05-f81e97afe6ef",
    "OCI": "612953c7-567a-58b3-bc42-ee817d2bbe74",
    "Sell Corp": "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
}

# Port IDs
PORTS = [
    "sg-sin",  # Singapore
    "nl-rtm",  # Rotterdam
    "us-hou",  # Houston
    "ae-fjr",  # Fujairah
    "es-alg",  # Algeciras
    "pa-pan",  # Panama
    "kr-bus",  # Busan
    "be-ant",  # Antwerp
    "jp-yok",  # Yokohama
    "hk-hkg",  # Hong Kong
    "my-klg",  # Port Klang
    "za-dur",  # Durban
    "br-sts",  # Santos
    "eg-psd",  # Port Said
    "gr-pir",  # Piraeus
    "mt-val",  # Valletta
    "gi-gib",  # Gibraltar
    "es-lpa",  # Las Palmas
]

# Port intelligence: congestion levels
CONGESTION_LEVELS = {
    # High
    "sg-sin": "High",
    "us-hou": "High",
    "be-ant": "High",
    "kr-bus": "High",
    # Moderate
    "nl-rtm": "Moderate",
    "ae-fjr": "Moderate",
    "hk-hkg": "Moderate",
    "my-klg": "Moderate",
    "jp-yok": "Moderate",
    "gr-pir": "Moderate",
    "br-sts": "Moderate",
    # Low
    "es-alg": "Low",
    "pa-pan": "Low",
    "za-dur": "Low",
    "eg-psd": "Low",
    "mt-val": "Low",
    "gi-gib": "Low",
    "es-lpa": "Low",
}

# Port regions for pricing
def get_region(port_id):
    """Map port ID to region for price adjustments."""
    asia = ["sg-sin", "hk-hkg", "my-klg", "jp-yok", "kr-bus"]
    europe = ["nl-rtm", "be-ant", "es-alg", "gr-pir", "mt-val", "gi-gib", "es-lpa"]
    americas = ["us-hou", "pa-pan", "br-sts"]
    middle_east = ["ae-fjr"]

    if port_id in asia:
        return "asia"
    elif port_id in europe:
        return "europe"
    elif port_id in americas:
        return "americas"
    elif port_id in middle_east:
        return "middle_east"
    else:
        return "global"

# Energy densities (MJ/kg)
ENERGY_DENSITIES = {
    "LSMGO": 42.7,
    "Methanol": 19.9,
    "Biofuel": 37.5,
    "LNG": 49.0,
    "Ammonia": 18.6,
}

# Product name mapping
PRODUCT_NAMES = {
    "LSMGO": "VLSFO 0.5%",
    "Methanol": "Green Methanol (ISCC)",
    "Biofuel": "Bio-FAME B30",
    "LNG": "LNG Bunker Grade",
    "Ammonia": "Green Ammonia",
}

def get_price_per_mt(fuel_type, port_id):
    """Get realistic price for fuel type and port region."""
    region = get_region(port_id)

    if fuel_type == "LSMGO":
        # Base $670, region adjustments
        base = Decimal("670")
        if region == "asia":
            return base + Decimal("50")  # Singapore premium ~$720
        elif region == "europe":
            return base + Decimal("20")  # Europe ~$690
        elif region == "americas":
            return base - Decimal("10")  # Houston ~$660
        elif region == "middle_east":
            return base + Decimal("30")
        return base

    elif fuel_type == "Methanol":
        base = Decimal("500")
        if region == "asia":
            return base + Decimal("70")  # $570
        elif region == "europe":
            return base + Decimal("60")  # $560
        elif region == "americas":
            return base + Decimal("65")  # $565
        elif region == "middle_east":
            return base + Decimal("75")  # $575
        return base

    elif fuel_type == "Biofuel":
        base = Decimal("1000")
        if region == "asia":
            return base + Decimal("100")  # $1100
        elif region == "europe":
            return base + Decimal("30")  # $1030
        elif region == "americas":
            return base + Decimal("50")  # $1050
        elif region == "middle_east":
            return base + Decimal("70")  # $1070
        return base

    elif fuel_type == "LNG":
        base = Decimal("950")
        if region == "asia":
            return base + Decimal("100")  # $1050
        elif region == "europe":
            return base + Decimal("50")  # $1000
        elif region == "americas":
            return base + Decimal("80")  # $1030
        elif region == "middle_east":
            return base + Decimal("60")  # $1010
        return base

    elif fuel_type == "Ammonia":
        base = Decimal("700")
        if region == "asia":
            return base + Decimal("50")  # $750
        elif region == "europe":
            return base + Decimal("30")  # $730
        elif region == "americas":
            return base + Decimal("40")  # $740
        elif region == "middle_east":
            return base + Decimal("60")  # $760
        return base

    return Decimal("500")

def get_methanol_price_avg(port_id):
    """Get average methanol price for port intelligence."""
    region = get_region(port_id)
    if region == "asia":
        return Decimal(str(random.uniform(550, 620)))
    elif region == "europe":
        return Decimal(str(random.uniform(500, 580)))
    elif region == "americas":
        return Decimal(str(random.uniform(510, 590)))
    elif region == "middle_east":
        return Decimal(str(random.uniform(520, 600)))
    return Decimal(str(random.uniform(530, 600)))

def get_biofuel_price_avg(port_id):
    """Get average biofuel price for port intelligence."""
    region = get_region(port_id)
    if region == "asia":
        return Decimal(str(random.uniform(1050, 1150)))
    elif region == "europe":
        return Decimal(str(random.uniform(980, 1050)))
    elif region == "americas":
        return Decimal(str(random.uniform(1000, 1080)))
    elif region == "middle_east":
        return Decimal(str(random.uniform(1020, 1100)))
    return Decimal(str(random.uniform(1000, 1100)))

def get_stock_levels(fuel_type):
    """Get realistic stock levels for fuel type."""
    if fuel_type == "LSMGO":
        current = random.randint(5000, 50000)
        incoming = random.randint(2000, 20000)
        reserved = random.randint(500, 5000)
    elif fuel_type == "Methanol":
        current = random.randint(3000, 30000)
        incoming = random.randint(1000, 15000)
        reserved = random.randint(200, 3000)
    elif fuel_type == "Biofuel":
        current = random.randint(1000, 15000)
        incoming = random.randint(500, 8000)
        reserved = random.randint(100, 2000)
    elif fuel_type == "LNG":
        current = random.randint(10000, 80000)
        incoming = random.randint(5000, 30000)
        reserved = random.randint(2000, 10000)
    elif fuel_type == "Ammonia":
        current = random.randint(2000, 20000)
        incoming = random.randint(1000, 10000)
        reserved = random.randint(200, 3000)
    else:
        current = incoming = reserved = 0

    return current, incoming, reserved

def is_certified(fuel_type):
    """Determine if item is certified. Green fuels 80%, conventional 30%."""
    if fuel_type in ["Methanol", "Ammonia", "Biofuel"]:
        return random.random() < 0.8
    else:  # LSMGO, LNG
        return random.random() < 0.3

def get_random_timestamp_within_7_days():
    """Get random timestamp within last 7 days."""
    days_back = random.randint(0, 7)
    hours_back = random.randint(0, 23)
    minutes_back = random.randint(0, 59)
    return datetime.utcnow() - timedelta(days=days_back, hours=hours_back, minutes=minutes_back)

def generate_inventory_items():
    """Generate ~85 inventory items with realistic supplier presence."""
    items = []

    # Supplier presence mapping: supplier -> [ports]
    supplier_ports = {
        "Vitol": [
            "sg-sin", "nl-rtm", "us-hou", "ae-fjr", "es-alg", "pa-pan",
            "kr-bus", "be-ant", "jp-yok", "hk-hkg", "my-klg", "za-dur"
        ],  # 12 ports
        "Trafigura": [
            "sg-sin", "nl-rtm", "us-hou", "ae-fjr", "kr-bus",
            "be-ant", "jp-yok", "hk-hkg", "br-sts", "gr-pir"
        ],  # 10 ports
        "Peninsula": [
            "sg-sin", "ae-fjr", "nl-rtm", "be-ant", "hk-hkg"
        ],  # 5 ports (SG, FUJ, RTM, ANT, HKG)
        "Bunker Holding": [
            "nl-rtm", "be-ant", "es-alg", "gr-pir", "mt-val",
            "sg-sin", "ae-fjr", "gi-gib"
        ],  # 8 ports (European + SG + FUJ)
        "OCI": [
            "nl-rtm", "sg-sin", "us-hou", "ae-fjr"
        ],  # 4 ports (RTM, SG, HOU, FUJ) - green fuels only
        "Sell Corp": [
            "sg-sin", "nl-rtm", "ae-fjr"
        ],  # 3 ports
    }

    # OCI: green fuels only (Methanol, Ammonia)
    oci_fuels = ["Methanol", "Ammonia"]

    # Peninsula: mainly LSMGO + Methanol
    peninsula_fuels = ["LSMGO", "LSMGO", "Methanol"]

    # Vitol, Trafigura, Bunker Holding: all fuels
    all_fuels = ["LSMGO", "Methanol", "Biofuel", "LNG", "Ammonia"]

    # Sell Corp: mixed
    sell_corp_fuels = ["LSMGO", "Methanol", "LNG"]

    for supplier_name, ports in supplier_ports.items():
        supplier_id = SUPPLIERS[supplier_name]

        # Determine which fuel types this supplier offers
        if supplier_name == "OCI":
            fuels = oci_fuels
        elif supplier_name == "Peninsula":
            fuels = peninsula_fuels
        elif supplier_name == "Sell Corp":
            fuels = sell_corp_fuels
        else:
            fuels = all_fuels

        # Generate items for each port-fuel combination
        for port_id in ports:
            # Generate 1-2 items per port per supplier
            num_items = random.randint(1, 2)

            for _ in range(num_items):
                fuel_type = random.choice(fuels)
                current, incoming, reserved = get_stock_levels(fuel_type)

                item = (
                    str(uuid.uuid4()),
                    supplier_id,
                    port_id,
                    fuel_type,
                    PRODUCT_NAMES[fuel_type],
                    current,
                    incoming,
                    reserved,
                    float(get_price_per_mt(fuel_type, port_id)),
                    float(ENERGY_DENSITIES[fuel_type]),
                    is_certified(fuel_type),
                    get_random_timestamp_within_7_days(),
                )
                items.append(item)

    return items

def generate_port_intelligence():
    """Generate port_intelligence rows for all ports with realistic data."""
    intelligence = []

    for port_id in PORTS:
        intel = (
            str(uuid.uuid4()),
            port_id,
            CONGESTION_LEVELS[port_id],
            float(get_methanol_price_avg(port_id)),
            float(get_biofuel_price_avg(port_id)),
            get_random_timestamp_within_7_days(),
        )
        intelligence.append(intel)

    return intelligence

def main():
    """Main seeding function."""
    print("Connecting to Verdaxis database...")
    conn = psycopg2.connect(**DB_CONFIG)
    conn.autocommit = False

    try:
        cur = conn.cursor()

        print("Clearing existing data...")
        cur.execute("DELETE FROM inventory_items;")
        cur.execute("DELETE FROM port_intelligence;")
        conn.commit()
        print("  ✓ Cleared inventory_items and port_intelligence")

        print("\nGenerating inventory items...")
        inventory_items = generate_inventory_items()
        print(f"  Generated {len(inventory_items)} inventory items")

        print("Inserting inventory items...")
        inventory_sql = """
            INSERT INTO inventory_items
            (id, supplier_id, port_id, fuel_type, product_name,
             current_stock_mt, incoming_stock_mt, reserved_stock_mt,
             price_per_mt_usd, energy_density_mj_kg, is_certified, updated_at)
            VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s)
        """

        execute_batch(cur, inventory_sql, inventory_items, page_size=100)
        conn.commit()
        print(f"  ✓ Inserted {len(inventory_items)} inventory items")

        print("\nGenerating port intelligence...")
        intelligence = generate_port_intelligence()
        print(f"  Generated {len(intelligence)} port intelligence records")

        print("Inserting port intelligence...")
        intelligence_sql = """
            INSERT INTO port_intelligence
            (id, port_id, congestion_level, methanol_price_avg, biofuel_price_avg, captured_at)
            VALUES (%s, %s, %s, %s, %s, %s)
        """

        execute_batch(cur, intelligence_sql, intelligence, page_size=100)
        conn.commit()
        print(f"  ✓ Inserted {len(intelligence)} port intelligence records")

        # Verify counts
        print("\nVerifying seeded data...")
        cur.execute("SELECT COUNT(*) FROM inventory_items;")
        inventory_count = cur.fetchone()[0]

        cur.execute("SELECT COUNT(*) FROM port_intelligence;")
        intelligence_count = cur.fetchone()[0]

        print(f"\nSeeding complete:")
        print(f"   inventory_items: {inventory_count} rows")
        print(f"   port_intelligence: {intelligence_count} rows")

        # Show breakdown by supplier
        print("\nInventory breakdown by supplier:")
        cur.execute("""
            SELECT supplier_id, COUNT(*) as count
            FROM inventory_items
            GROUP BY supplier_id
            ORDER BY count DESC;
        """)

        for supplier_id, count in cur.fetchall():
            supplier_name = [k for k, v in SUPPLIERS.items() if v == supplier_id][0]
            print(f"   {supplier_name}: {count} items")

        # Show breakdown by fuel type
        print("\nInventory breakdown by fuel type:")
        cur.execute("""
            SELECT fuel_type, COUNT(*) as count
            FROM inventory_items
            GROUP BY fuel_type
            ORDER BY count DESC;
        """)

        for fuel_type, count in cur.fetchall():
            print(f"   {fuel_type}: {count} items")

        # Show port coverage
        print("\nPort intelligence coverage:")
        cur.execute("""
            SELECT COUNT(DISTINCT port_id) FROM port_intelligence;
        """)
        port_coverage = cur.fetchone()[0]
        print(f"   {port_coverage}/{len(PORTS)} ports with intelligence")

        cur.close()

    except Exception as e:
        conn.rollback()
        print(f"Error during seeding: {e}")
        raise
    finally:
        conn.close()

if __name__ == "__main__":
    main()
