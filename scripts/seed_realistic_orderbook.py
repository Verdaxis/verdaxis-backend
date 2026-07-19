#!/usr/bin/env python3
"""
Seed realistic bunker fuel marketplace data for Verdaxis Exchange.

Pricing references (2026 market):
- Methanol (Green): $480-700/MT region-dependent
- LNG (Conventional): $800-1200/MT
- Biofuel (Bio/FAME): $900-1300/MT
- Ammonia (Green): $600-900/MT
- VLSFO (Conventional): $520-620/MT
- VLSFO (Green): $700-850/MT
- MGO (Conventional): $650-780/MT
- MGO (Bio): $950-1200/MT
- Hydrogen (Green): $3500-5500/MT (early market, low volumes)

Spread structure: asks are above bids (realistic non-crossed market).
Typical spread: 2-8% of mid-price.
"""

import psycopg2
import uuid
import random
import json
from datetime import datetime, timedelta, date
from app.seeds.safety import canonical_seed_window, seed_connection

# Database connection
conn = seed_connection()
conn.autocommit = False
cur = conn.cursor()

# ── Reference data ──────────────────────────────────────────────────────

PRODUCTS = {
    "methanol_green":  "b0f9b249-1ae4-5e02-adf5-e4964788ad8e",
    "lng_conv":        "758cb4b6-463f-5431-8196-17037b4e015f",
    "biofuel_bio":     "3ebf5484-430e-50b7-be68-04cdd39f8c0d",
    "ammonia_green":   "57015681-f987-556b-9711-97524da07f63",
    "vlsfo_conv":      "85f9e6fd-9ef6-50bf-b0aa-323c71efee55",
    "vlsfo_green":     "8210134a-4d89-5bb7-a07c-6820100dbedf",
    "mgo_conv":        "a7c823a5-03b0-54a7-9df3-0c78b76e6ea7",
    "mgo_bio":         "03f3896b-ff8b-533f-9582-a521cf9b94ca",
    "hydrogen_green":  "e8774dde-1ecb-51bd-8849-54718d7d36ec",
}

DELIVERY_POINTS = {
    "singapore":  "73835e92-820e-584b-8280-bb61c63aa28e",
    "rotterdam":  "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36",
    "ara":        "0f6b6006-61ef-5ef9-b096-71bf87d1d3d7",
    "fujairah":   "f4877150-d88e-5825-b154-3410dfc9f1f1",
    "houston":    "a083db06-b050-56c2-a274-3897eac2fdae",
}

ORGS = {
    "green_marine": "90def6b3-4130-45ce-a9b9-314fa139f8e2",
    "marinachain":  "97d4727d-fe17-4be8-bd2b-66653a4f8b76",
    "buy_corp":     "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "sell_corp":    "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
}

SUPPLIER_ORGS = ["green_marine", "sell_corp", "marinachain"]
BUYER_ORGS = ["buy_corp", "marinachain", "green_marine"]

AVAILABILITY_WINDOWS = [
    canonical_seed_window(value)
    for value in ("SPOT", "2026-Q2", "2026-Q3", "2026-Q4", "2027-CAL")
]

CERTIFICATIONS_POOL = [
    ["ISCC"], ["RSB"], ["ISCC", "RSB"], ["ISCC", "EU-RED"],
    ["RSB", "CORSIA"], [], [], [],
]

# ── Pricing model ───────────────────────────────────────────────────────
# (mid_price, spread_pct, min_lot, max_lot)

PRICING = {
    "methanol_green": {
        "singapore":  (565, 0.04, 1000, 5000),
        "rotterdam":  (610, 0.035, 1000, 5000),
        "ara":        (605, 0.035, 1000, 5000),
        "fujairah":   (540, 0.04, 500, 3000),
        "houston":    (590, 0.04, 1000, 5000),
    },
    "lng_conv": {
        "singapore":  (1050, 0.03, 2000, 10000),
        "rotterdam":  (980, 0.035, 2000, 8000),
        "fujairah":   (920, 0.04, 1000, 5000),
        "houston":    (850, 0.03, 2000, 10000),
    },
    "biofuel_bio": {
        "singapore":  (1120, 0.035, 500, 3000),
        "rotterdam":  (1050, 0.03, 500, 3000),
        "ara":        (1060, 0.03, 500, 3000),
        "houston":    (1150, 0.04, 500, 2000),
    },
    "ammonia_green": {
        "singapore":  (780, 0.04, 1000, 5000),
        "rotterdam":  (720, 0.035, 1000, 5000),
        "fujairah":   (690, 0.04, 500, 3000),
        "houston":    (750, 0.045, 1000, 5000),
    },
    "vlsfo_conv": {
        "singapore":  (578, 0.025, 500, 8000),
        "rotterdam":  (555, 0.025, 500, 8000),
        "ara":        (560, 0.025, 500, 8000),
        "fujairah":   (545, 0.03, 500, 5000),
        "houston":    (540, 0.025, 500, 8000),
    },
    "vlsfo_green": {
        "singapore":  (790, 0.035, 500, 3000),
        "rotterdam":  (760, 0.035, 500, 3000),
        "ara":        (765, 0.035, 500, 3000),
    },
    "mgo_conv": {
        "singapore":  (720, 0.025, 200, 3000),
        "rotterdam":  (695, 0.025, 200, 3000),
        "ara":        (700, 0.025, 200, 3000),
        "fujairah":   (680, 0.03, 200, 2000),
        "houston":    (670, 0.025, 200, 3000),
    },
    "mgo_bio": {
        "singapore":  (1080, 0.035, 200, 1500),
        "rotterdam":  (1020, 0.03, 200, 1500),
        "ara":        (1030, 0.03, 200, 1500),
    },
    "hydrogen_green": {
        "singapore":  (4800, 0.05, 50, 500),
        "rotterdam":  (4500, 0.05, 50, 500),
    },
}


def rand_quantity(min_lot, max_lot):
    if max_lot <= 500:
        return round(random.uniform(min_lot, max_lot) / 10) * 10
    elif max_lot <= 2000:
        return round(random.uniform(min_lot, max_lot) / 50) * 50
    else:
        return round(random.uniform(min_lot, max_lot) / 100) * 100


def rand_price_around(mid, spread_pct, side):
    if side == "ASK":
        factor = 1 + spread_pct * random.uniform(0.5, 1.5)
    else:
        factor = 1 - spread_pct * random.uniform(0.5, 1.5)
    price = mid * factor
    return round(price * 2) / 2


def random_created_at():
    days_ago = random.expovariate(0.3)
    days_ago = min(days_ago, 14)
    return datetime.utcnow() - timedelta(
        days=days_ago, hours=random.randint(0, 23), minutes=random.randint(0, 59)
    )


def build_order(side, product_key, dp_key, mid, spread_pct, min_lot, max_lot):
    qty = rand_quantity(min_lot, max_lot)
    price = rand_price_around(mid, spread_pct, side)

    status = "OPEN"
    remaining = qty
    if random.random() < 0.15:
        status = "PARTIALLY_FILLED"
        filled_pct = random.uniform(0.1, 0.6)
        filled = round(qty * filled_pct / 50) * 50
        if filled < 50:
            filled = 50
        remaining = qty - filled

    window = random.choices(
        AVAILABILITY_WINDOWS, weights=[40, 25, 15, 10, 10], k=1
    )[0]

    certs = []
    if side == "ASK" and random.random() < 0.30:
        certs = random.choice(CERTIFICATIONS_POOL)

    verified = side == "ASK" and random.random() < 0.20

    if side == "ASK":
        org_key = random.choice(SUPPLIER_ORGS)
    else:
        org_key = random.choice(BUYER_ORGS)

    dw_start = None
    dw_end = None
    if window != "SPOT":
        if window == "2026-Q2":
            dw_start, dw_end = date(2026, 4, 1), date(2026, 6, 30)
        elif window == "2026-Q3":
            dw_start, dw_end = date(2026, 7, 1), date(2026, 9, 30)
        elif window == "2026-Q4":
            dw_start, dw_end = date(2026, 10, 1), date(2026, 12, 31)
        elif window == "2027-CAL":
            dw_start, dw_end = date(2027, 1, 1), date(2027, 12, 31)

    expires_at = None
    if random.random() < 0.30:
        expires_at = datetime.utcnow() + timedelta(days=random.randint(7, 30))

    ci_gco2 = None
    energy_mj = None
    if side == "ASK" and random.random() < 0.60:
        if "methanol" in product_key:
            ci_gco2 = round(random.uniform(10, 30), 2)
            energy_mj = round(random.uniform(19, 21), 2)
        elif "ammonia" in product_key:
            ci_gco2 = round(random.uniform(5, 20), 2)
            energy_mj = round(random.uniform(18, 19), 2)
        elif "hydrogen" in product_key:
            ci_gco2 = round(random.uniform(3, 15), 2)
            energy_mj = round(random.uniform(119, 121), 2)
        elif "biofuel" in product_key or "mgo_bio" in product_key:
            ci_gco2 = round(random.uniform(20, 45), 2)
            energy_mj = round(random.uniform(35, 38), 2)
        elif "vlsfo_green" in product_key:
            ci_gco2 = round(random.uniform(50, 70), 2)
            energy_mj = round(random.uniform(40, 42), 2)

    return {
        "id": str(uuid.uuid4()),
        "organization_id": ORGS[org_key],
        "side": side,
        "product_id": PRODUCTS[product_key],
        "delivery_point_id": DELIVERY_POINTS[dp_key],
        "quantity_mt": qty,
        "remaining_quantity_mt": remaining,
        "price_per_mt_usd": price,
        "availability_window": window,
        "delivery_window_start": dw_start,
        "delivery_window_end": dw_end,
        "certifications": certs if certs else [],
        "is_verdaxis_verified": verified,
        "carbon_intensity_gco2_mj": ci_gco2,
        "energy_density_mj_kg": energy_mj,
        "status": status,
        "expires_at": expires_at,
        "created_at": random_created_at(),
    }


# ── Generate orders ─────────────────────────────────────────────────────

orders = []

ask_count = 0
for product_key, regions in PRICING.items():
    for dp_key, (mid, spread, min_lot, max_lot) in regions.items():
        n_asks = random.choices([1, 2, 3], weights=[40, 40, 20], k=1)[0]
        for _ in range(n_asks):
            orders.append(build_order("ASK", product_key, dp_key, mid, spread, min_lot, max_lot))
            ask_count += 1

bid_count = 0
for product_key, regions in PRICING.items():
    for dp_key, (mid, spread, min_lot, max_lot) in regions.items():
        n_bids = random.choices([0, 1, 2], weights=[30, 50, 20], k=1)[0]
        for _ in range(n_bids):
            orders.append(build_order("BID", product_key, dp_key, mid, spread, min_lot, max_lot))
            bid_count += 1


# ── Cancel old unrealistic orders ───────────────────────────────────────

print("Cancelling old seed data...")
cur.execute("""
    UPDATE orderbook_orders
    SET status = 'CANCELLED', updated_at = NOW()
    WHERE status IN ('OPEN', 'PARTIALLY_FILLED')
""")
cancelled = cur.rowcount
print(f"  Cancelled {cancelled} old orders")


# ── Insert new orders ───────────────────────────────────────────────────

print(f"\nInserting {len(orders)} realistic orders ({ask_count} ASK, {bid_count} BID)...")

INSERT_SQL = """
    INSERT INTO orderbook_orders (
        id, organization_id, side, product_id, delivery_point_id,
        quantity_mt, remaining_quantity_mt, price_per_mt_usd,
        availability_window, delivery_window_start, delivery_window_end,
        certifications, is_verdaxis_verified,
        carbon_intensity_gco2_mj, energy_density_mj_kg,
        status, expires_at, created_at, updated_at
    ) VALUES (
        %(id)s, %(organization_id)s, %(side)s, %(product_id)s, %(delivery_point_id)s,
        %(quantity_mt)s, %(remaining_quantity_mt)s, %(price_per_mt_usd)s,
        %(availability_window)s, %(delivery_window_start)s, %(delivery_window_end)s,
        %(certifications)s::jsonb, %(is_verdaxis_verified)s,
        %(carbon_intensity_gco2_mj)s, %(energy_density_mj_kg)s,
        %(status)s, %(expires_at)s, %(created_at)s, %(created_at)s
    )
"""

for order in orders:
    params = dict(order)
    params["certifications"] = json.dumps(params["certifications"])
    cur.execute(INSERT_SQL, params)

conn.commit()

# ── Summary ─────────────────────────────────────────────────────────────

print("\n== Seed Summary ==")

cur.execute("""
    SELECT side, count(*),
           min(price_per_mt_usd)::numeric(10,2),
           max(price_per_mt_usd)::numeric(10,2),
           avg(price_per_mt_usd)::numeric(10,2)
    FROM orderbook_orders
    WHERE status IN ('OPEN', 'PARTIALLY_FILLED')
    GROUP BY side ORDER BY side
""")
for row in cur.fetchall():
    print(f"  {row[0]}: {row[1]} orders | ${row[2]} - ${row[3]} (avg ${row[4]})")

print("\n== By Product & Side ==")
cur.execute("""
    SELECT p.name, o.side, count(*),
           min(o.price_per_mt_usd)::numeric(10,2),
           max(o.price_per_mt_usd)::numeric(10,2)
    FROM orderbook_orders o
    JOIN products p ON o.product_id = p.id
    WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
    GROUP BY p.name, o.side
    ORDER BY p.name, o.side
""")
for row in cur.fetchall():
    print(f"  {row[0]:20s} {row[1]:3s}: {row[2]:3d} orders  ${str(row[3]):>8s} - ${str(row[4]):>8s}")

print("\n== Spread Check (non-crossed market) ==")
cur.execute("""
    WITH best AS (
        SELECT p.name AS product,
               dp.name AS location,
               o.product_id, o.delivery_point_id,
               min(CASE WHEN o.side='ASK' THEN o.price_per_mt_usd END) AS best_ask,
               max(CASE WHEN o.side='BID' THEN o.price_per_mt_usd END) AS best_bid
        FROM orderbook_orders o
        JOIN products p ON o.product_id = p.id
        LEFT JOIN delivery_points dp ON o.delivery_point_id = dp.id
        WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
        GROUP BY p.name, dp.name, o.product_id, o.delivery_point_id
    )
    SELECT product, location,
           best_bid::numeric(10,2),
           best_ask::numeric(10,2),
           CASE WHEN best_bid IS NOT NULL AND best_ask IS NOT NULL
                THEN CASE WHEN best_bid >= best_ask THEN 'CROSSED!' ELSE 'OK' END
                ELSE 'one-sided'
           END AS status
    FROM best ORDER BY product, location
""")
crossed_count = 0
crossed_pairs = []
for row in cur.fetchall():
    marker = ""
    if row[4] == "CROSSED!":
        crossed_count += 1
        marker = " *** FIX NEEDED ***"
    print(f"  {row[0]:20s} @ {row[1]:12s}  bid={str(row[2] or 'N/A'):>8s}  ask={str(row[3] or 'N/A'):>8s}  {row[4]}{marker}")

if crossed_count > 0:
    print(f"\n  WARNING: {crossed_count} pairs crossed. Lowering bids by 6%...")
    cur.execute("""
        WITH crossed AS (
            SELECT o.product_id, o.delivery_point_id
            FROM orderbook_orders o
            WHERE o.status IN ('OPEN', 'PARTIALLY_FILLED')
            GROUP BY o.product_id, o.delivery_point_id
            HAVING max(CASE WHEN o.side='BID' THEN o.price_per_mt_usd END)
                >= min(CASE WHEN o.side='ASK' THEN o.price_per_mt_usd END)
        )
        UPDATE orderbook_orders o
        SET price_per_mt_usd = price_per_mt_usd * 0.94,
            updated_at = NOW()
        FROM crossed c
        WHERE o.product_id = c.product_id
          AND o.delivery_point_id = c.delivery_point_id
          AND o.side = 'BID'
          AND o.status IN ('OPEN', 'PARTIALLY_FILLED')
    """)
    conn.commit()
    print(f"  Adjusted {cur.rowcount} bid orders")

cur.execute("""
    SELECT count(*) FILTER (WHERE status = 'OPEN') AS open_ct,
           count(*) FILTER (WHERE status = 'PARTIALLY_FILLED') AS partial_ct,
           count(*) FILTER (WHERE certifications::text != '[]') AS with_certs,
           count(*) FILTER (WHERE is_verdaxis_verified) AS verified_ct,
           count(*) FILTER (WHERE carbon_intensity_gco2_mj IS NOT NULL) AS with_ci
    FROM orderbook_orders
    WHERE status IN ('OPEN', 'PARTIALLY_FILLED')
""")
row = cur.fetchone()
print(f"\n== Order Characteristics ==")
print(f"  Open: {row[0]}, Partially Filled: {row[1]}")
print(f"  With certifications: {row[2]}")
print(f"  Verdaxis verified: {row[3]}")
print(f"  With CI data: {row[4]}")

cur.close()
conn.close()
print("\nDone! Realistic orderbook seeded successfully.")
