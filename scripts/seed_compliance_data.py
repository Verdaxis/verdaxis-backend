#!/usr/bin/env python3
"""
Seed script for compliance_ledger table.
Populates the table with realistic EU ETS, FuelEU Maritime, and Pooling Transfer entries.
"""

import psycopg2
from decimal import Decimal
from datetime import datetime, timedelta
import uuid
from random import randint, choice, uniform
from app.seeds.safety import seed_connection

# Database connection
conn = seed_connection()
conn.autocommit = False

cursor = conn.cursor()

# Organization IDs
ORGANIZATIONS = {
    "buy_corp": "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "maersk": "4da7b285-34ee-5443-9406-f96b4ed1a251",
    "evergreen": "0dbce576-2026-5925-ab66-674d505e98ad",
    "cosco": "3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a",
    "msc": "277491df-cb0d-5f2d-a2cf-5746829c6da6",
    "cmacgm": "3b302066-d65c-5c3e-8fcc-70b3da3bcafd",
    "marinachain": "97d4727d-fe17-4be8-bd2b-66653a4f8b76",
}

ORG_NAMES = {
    "buy_corp": "Buy Corp",
    "maersk": "Maersk",
    "evergreen": "Evergreen",
    "cosco": "COSCO",
    "msc": "MSC",
    "cmacgm": "CMA CGM",
    "marinachain": "MarinaChain",
}

ORG_SHORTS = {
    "buy_corp": "BC",
    "maersk": "MAE",
    "evergreen": "EVG",
    "cosco": "COS",
    "msc": "MSC",
    "cmacgm": "CMG",
    "marinachain": "MCH",
}

# Data structures
compliance_entries = []

def add_entry(org_key, transaction_type, amount, currency, units, description, reference_id, created_at):
    """Add a compliance ledger entry."""
    compliance_entries.append({
        "id": str(uuid.uuid4()),
        "organization_id": ORGANIZATIONS[org_key],
        "transaction_type": transaction_type,
        "amount": amount,
        "currency": currency,
        "units": units,
        "description": description,
        "reference_id": reference_id,
        "created_at": created_at,
    })

# ============================================================================
# EU ETS ALLOWANCE PURCHASES (EUA_Purchase) — 18 entries
# ============================================================================
print("Generating EU ETS EUA Purchase entries...")

aua_price_range = (65, 85)  # EUR per tonne CO2 in Q1 2026
purchase_qty_by_size = {
    "large": (25000, 50000),      # Maersk, MSC, CMA CGM
    "medium": (15000, 35000),     # COSCO, Evergreen
    "small": (5000, 15000),       # Buy Corp, MarinaChain
}

org_sizes = {
    "maersk": "large",
    "msc": "large",
    "cmacgm": "large",
    "cosco": "medium",
    "evergreen": "medium",
    "buy_corp": "small",
    "marinachain": "small",
}

vessel_counts = {
    "maersk": 45,
    "msc": 50,
    "cmacgm": 40,
    "cosco": 28,
    "evergreen": 25,
    "buy_corp": 8,
    "marinachain": 5,
}

# Q4 2025 purchases (Oct-Dec 2025)
q4_2025_dates = [
    datetime(2025, 10, 15),
    datetime(2025, 11, 10),
    datetime(2025, 12, 5),
]

# Q1 2026 purchases (Jan-Mar 2026)
q1_2026_dates = [
    datetime(2026, 1, 10),
    datetime(2026, 2, 15),
    datetime(2026, 3, 12),
]

eua_counter = 0
for org_key, org_size in org_sizes.items():
    min_qty, max_qty = purchase_qty_by_size[org_size]

    # 3 entries per org (Q4 2025 x1, Q1 2026 x2)
    # Q4 2025
    qty_q4 = randint(min_qty, max_qty)
    price_per_eua = Decimal(str(round(uniform(aua_price_range[0], aua_price_range[1]), 2)))
    amount_q4 = Decimal(qty_q4) * price_per_eua
    vessel_count = vessel_counts[org_key]

    add_entry(
        org_key=org_key,
        transaction_type="EUA_Purchase",
        amount=amount_q4,
        currency="EUR",
        units=Decimal(qty_q4),
        description=f"Q4 2025 EU ETS allowance purchase - {vessel_count} vessels covered",
        reference_id=f"ETS-2025-Q4-{ORG_SHORTS[org_key]}",
        created_at=q4_2025_dates[0],
    )
    eua_counter += 1

    # Q1 2026 - 2 entries
    for quarter_idx, q1_date in enumerate(q1_2026_dates[1:], 1):
        qty_q1 = randint(min_qty, max_qty)
        price_per_eua = Decimal(str(round(uniform(aua_price_range[0], aua_price_range[1]), 2)))
        amount_q1 = Decimal(qty_q1) * price_per_eua

        add_entry(
            org_key=org_key,
            transaction_type="EUA_Purchase",
            amount=amount_q1,
            currency="EUR",
            units=Decimal(qty_q1),
            description=f"Q{quarter_idx + 3} 2025 EU ETS allowance purchase - {vessel_count} vessels covered",
            reference_id=f"ETS-2026-Q{quarter_idx}-{ORG_SHORTS[org_key]}",
            created_at=q1_date,
        )
        eua_counter += 1

# ============================================================================
# FUELEU MARITIME PENALTIES (FuelEU_Penalty) — 8 entries
# ============================================================================
print("Generating FuelEU Maritime Penalty entries...")

# Only some companies get penalties
penalty_orgs = [
    "cosco",      # Q3 2025
    "evergreen",  # Q3 2025
    "buy_corp",   # Q4 2025
    "marinachain",# Q4 2025
    "msc",        # Q4 2025
    "cmacgm",     # Q1 2026
    "maersk",     # Q1 2026
    "cosco",      # Q1 2026 (second penalty)
]

penalty_dates = [
    datetime(2025, 9, 20),   # COSCO Q3
    datetime(2025, 9, 25),   # Evergreen Q3
    datetime(2025, 10, 15),  # Buy Corp Q4
    datetime(2025, 11, 5),   # MarinaChain Q4
    datetime(2025, 11, 20),  # MSC Q4
    datetime(2026, 1, 30),   # CMA CGM Q1
    datetime(2026, 2, 28),   # Maersk Q1
    datetime(2026, 3, 15),   # COSCO Q1
]

penalty_amounts = [
    Decimal("125000"),
    Decimal("95000"),
    Decimal("65000"),
    Decimal("55000"),
    Decimal("180000"),
    Decimal("145000"),
    Decimal("190000"),
    Decimal("110000"),
]

ghg_excesses = [
    "12.5",
    "10.2",
    "8.7",
    "7.3",
    "15.8",
    "13.6",
    "17.2",
    "11.9",
]

penalty_counter = 0
for org_key, amount, ghg_excess, penalty_date in zip(penalty_orgs, penalty_amounts, ghg_excesses, penalty_dates):
    add_entry(
        org_key=org_key,
        transaction_type="FuelEU_Penalty",
        amount=amount,
        currency="EUR",
        units=None,
        description=f"FuelEU Maritime compliance deficit - bulk carrier fleet - GHG intensity excess {ghg_excess} gCO2eq/MJ",
        reference_id=f"FUELEU-2025-{ORG_SHORTS[org_key]}-{penalty_counter + 1}",
        created_at=penalty_date,
    )
    penalty_counter += 1

# ============================================================================
# POOLING TRANSFERS (Pooling_Transfer) — 9 entries
# ============================================================================
print("Generating FuelEU Pooling Transfer entries...")

pooling_partners = [
    ("maersk", "evergreen", 250000, True),      # Maersk sells to Evergreen
    ("msc", "buy_corp", 180000, True),           # MSC sells to Buy Corp
    ("cmacgm", "cosco", 320000, True),           # CMA CGM sells to COSCO
    ("evergreen", "marinachain", 95000, True),   # Evergreen sells to MarinaChain
    ("maersk", "cmacgm", 410000, False),         # Maersk buys from CMA CGM
    ("cosco", "msc", 225000, False),             # COSCO buys from MSC
    ("buy_corp", "maersk", 140000, False),       # Buy Corp buys from Maersk
    ("marinachain", "evergreen", 75000, False),  # MarinaChain buys from Evergreen
    ("cmacgm", "msc", 180000, True),             # CMA CGM sells to MSC
]

pooling_dates = [
    datetime(2026, 1, 8),
    datetime(2026, 1, 15),
    datetime(2026, 1, 22),
    datetime(2026, 2, 5),
    datetime(2026, 2, 12),
    datetime(2026, 2, 19),
    datetime(2026, 3, 1),
    datetime(2026, 3, 8),
    datetime(2026, 3, 20),
]

pooling_counter = 0
for (seller_key, buyer_key, amount, is_transfer), pool_date in zip(pooling_partners, pooling_dates):
    transfer_type = "transfer" if is_transfer else "receipt"

    add_entry(
        org_key=seller_key if is_transfer else buyer_key,
        transaction_type="Pooling_Transfer",
        amount=Decimal(amount) if is_transfer else Decimal(-amount),
        currency="EUR",
        units=None,
        description=f"FuelEU pooling {transfer_type} - {randint(500, 2000)} compliance units - Partner: {ORG_NAMES[buyer_key] if is_transfer else ORG_NAMES[seller_key]}",
        reference_id=f"POOL-2026-{pooling_counter + 1:02d}",
        created_at=pool_date,
    )
    pooling_counter += 1

# ============================================================================
# INSERT INTO DATABASE
# ============================================================================
print(f"\nTotal entries to insert: {len(compliance_entries)}")
print(f"  - EUA Purchases: {eua_counter}")
print(f"  - FuelEU Penalties: {penalty_counter}")
print(f"  - Pooling Transfers: {pooling_counter}")

# Clear existing data
print("\nClearing existing compliance_ledger entries...")
cursor.execute("DELETE FROM compliance_ledger;")

# Insert entries
insert_sql = """
INSERT INTO compliance_ledger
(id, organization_id, transaction_type, amount, currency, units, description, reference_id, created_at)
VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
"""

inserted_count = 0
for entry in compliance_entries:
    cursor.execute(insert_sql, (
        entry["id"],
        entry["organization_id"],
        entry["transaction_type"],
        entry["amount"],
        entry["currency"],
        entry["units"],
        entry["description"],
        entry["reference_id"],
        entry["created_at"],
    ))
    inserted_count += 1

conn.commit()
print(f"\nSuccessfully inserted {inserted_count} compliance ledger entries.")

# ============================================================================
# SUMMARY STATISTICS
# ============================================================================
print("\n" + "="*70)
print("COMPLIANCE LEDGER SEED SUMMARY")
print("="*70)

cursor.execute("""
SELECT
    transaction_type,
    COUNT(*) as count,
    SUM(amount) as total_amount,
    AVG(amount) as avg_amount,
    MIN(amount) as min_amount,
    MAX(amount) as max_amount
FROM compliance_ledger
GROUP BY transaction_type
ORDER BY transaction_type
""")

print("\nBy Transaction Type:")
print("-" * 70)
for row in cursor.fetchall():
    trans_type, count, total, avg, min_amt, max_amt = row
    print(f"{trans_type:20s} | Count: {count:2d} | Total: €{float(total):>12,.2f} | "
          f"Avg: €{float(avg):>10,.2f} | Range: €{float(min_amt):>10,.2f} - €{float(max_amt):>10,.2f}")

cursor.execute("""
SELECT
    organization_id,
    COUNT(*) as count,
    SUM(amount) as total_amount
FROM compliance_ledger
GROUP BY organization_id
ORDER BY total_amount DESC
""")

print("\nBy Organization:")
print("-" * 70)
total_all = Decimal("0")
for row in cursor.fetchall():
    org_id, count, total = row
    org_name = next((name for key, uid in ORGANIZATIONS.items() if str(uid) == str(org_id)
                     for name in [ORG_NAMES[key]]), "Unknown")
    print(f"{org_name:15s} | Count: {count:2d} | Total: €{float(total):>12,.2f}")
    total_all += Decimal(str(total))

cursor.execute("SELECT COUNT(*) FROM compliance_ledger")
total_entries = cursor.fetchone()[0]

print("\n" + "="*70)
print(f"Total Entries: {total_entries}")
print(f"Total Amount (all currencies): €{float(total_all):,.2f}")
print("="*70)

cursor.close()
conn.close()
print("\nDatabase connection closed. Seed complete!")
