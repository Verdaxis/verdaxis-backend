#!/usr/bin/env python3
"""
Seed user activity data for Verdaxis platform:
- Watchlists with entries
- Price alerts
- Match suggestions

This script is idempotent and will clean existing data before seeding.
"""

import psycopg2
import uuid
import json
import random
from datetime import datetime, timedelta
from app.seeds.safety import seed_connection

# Database connection
conn = seed_connection()
conn.autocommit = False
cur = conn.cursor()

# ── Reference data ──────────────────────────────────────────────────────

USERS = {
    "buyer@buy.com": "37c639be-8b49-4981-8d86-c7f2ef83bec3",
    "seller@sell.com": "11785ff3-3753-4fa5-93e1-d815f5c4a4b3",
    "jon@marinachain.io": "ece98943-417e-4ef0-935b-d45956becf7b",
    "admin@verdaxis.com": "aee49e44-cae4-498a-8a11-f94082a932dd",
}

ORGS = {
    "buyer@buy.com": "acc3f20a-fe94-4463-9029-a55e35634eb7",
    "seller@sell.com": "c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4",
    "jon@marinachain.io": "97d4727d-fe17-4be8-bd2b-66653a4f8b76",
    "admin@verdaxis.com": "97d4727d-fe17-4be8-bd2b-66653a4f8b76",
}

PRODUCTS = {
    "Ammonia Green": "57015681-f987-556b-9711-97524da07f63",
    "Biofuel Bio": "3ebf5484-430e-50b7-be68-04cdd39f8c0d",
    "Hydrogen Green": "e8774dde-1ecb-51bd-8849-54718d7d36ec",
    "LNG Conventional": "758cb4b6-463f-5431-8196-17037b4e015f",
    "Methanol Green": "b0f9b249-1ae4-5e02-adf5-e4964788ad8e",
    "MGO Bio": "03f3896b-ff8b-533f-9582-a521cf9b94ca",
    "MGO Conventional": "a7c823a5-03b0-54a7-9df3-0c78b76e6ea7",
    "VLSFO Conventional": "85f9e6fd-9ef6-50bf-b0aa-323c71efee55",
    "VLSFO Green": "8210134a-4d89-5bb7-a07c-6820100dbedf",
}

DELIVERY_POINTS = {
    "ARA": "0f6b6006-61ef-5ef9-b096-71bf87d1d3d7",
    "Fujairah": "f4877150-d88e-5825-b154-3410dfc9f1f1",
    "Houston": "a083db06-b050-56c2-a274-3897eac2fdae",
    "Rotterdam": "1379d36c-1ca9-55b7-9c0d-5235a0ba1f36",
    "Singapore": "73835e92-820e-584b-8280-bb61c63aa28e",
}

# ── Clean existing data ──────────────────────────────────────────────────

print("Cleaning existing user activity data...")
cur.execute("DELETE FROM watchlist_entries")
cur.execute("DELETE FROM watchlists")
cur.execute("DELETE FROM price_alerts")
cur.execute("DELETE FROM match_suggestions")
conn.commit()
print("  Cleaned all tables")


# ── Watchlists ──────────────────────────────────────────────────────────

print("\nSeeding watchlists...")

watchlists_data = [
    {
        "user_id": USERS["buyer@buy.com"],
        "name": "Singapore Green Fuels",
        "entries": [
            ("Methanol Green", "Singapore"),
            ("Biofuel Bio", "Singapore"),
        ]
    },
    {
        "user_id": USERS["buyer@buy.com"],
        "name": "European VLSFO",
        "entries": [
            ("VLSFO Conventional", "Rotterdam"),
            ("VLSFO Conventional", "ARA"),
            ("VLSFO Green", "Rotterdam"),
            ("VLSFO Green", "ARA"),
        ]
    },
    {
        "user_id": USERS["buyer@buy.com"],
        "name": "LNG Watch",
        "entries": [
            ("LNG Conventional", "Singapore"),
            ("LNG Conventional", "Fujairah"),
        ]
    },
    {
        "user_id": USERS["jon@marinachain.io"],
        "name": "Green Transition Fuels",
        "entries": [
            ("Methanol Green", None),
            ("Ammonia Green", None),
            ("Hydrogen Green", None),
        ]
    },
    {
        "user_id": USERS["jon@marinachain.io"],
        "name": "Singapore Hub",
        "entries": [
            ("Ammonia Green", "Singapore"),
            ("Biofuel Bio", "Singapore"),
            ("Hydrogen Green", "Singapore"),
            ("LNG Conventional", "Singapore"),
            ("Methanol Green", "Singapore"),
            ("MGO Bio", "Singapore"),
            ("MGO Conventional", "Singapore"),
            ("VLSFO Conventional", "Singapore"),
            ("VLSFO Green", "Singapore"),
        ]
    },
    {
        "user_id": USERS["seller@sell.com"],
        "name": "Competitor Pricing ARA",
        "entries": [
            ("VLSFO Conventional", "ARA"),
            ("MGO Conventional", "ARA"),
        ]
    },
    {
        "user_id": USERS["seller@sell.com"],
        "name": "Green Premium Watch",
        "entries": [
            ("VLSFO Green", "Rotterdam"),
            ("MGO Bio", "Rotterdam"),
            ("Methanol Green", "Rotterdam"),
        ]
    },
    {
        "user_id": USERS["admin@verdaxis.com"],
        "name": "Platform Overview",
        "entries": [
            ("VLSFO Conventional", None),
            ("Methanol Green", None),
            ("Ammonia Green", None),
        ]
    },
]

watchlist_count = 0
entry_count = 0

for wl_data in watchlists_data:
    wl_id = str(uuid.uuid4())
    now = datetime.utcnow()

    cur.execute(
        """
        INSERT INTO watchlists (id, user_id, name, created_at)
        VALUES (%s, %s, %s, %s)
        """,
        (wl_id, wl_data["user_id"], wl_data["name"], now)
    )
    watchlist_count += 1

    # Insert entries
    for product_name, delivery_point_name in wl_data["entries"]:
        entry_id = str(uuid.uuid4())
        product_id = PRODUCTS[product_name]
        delivery_point_id = DELIVERY_POINTS[delivery_point_name] if delivery_point_name else None

        cur.execute(
            """
            INSERT INTO watchlist_entries (id, watchlist_id, product_id, delivery_point_id, created_at)
            VALUES (%s, %s, %s, %s, %s)
            """,
            (entry_id, wl_id, product_id, delivery_point_id, now)
        )
        entry_count += 1

conn.commit()
print(f"  Created {watchlist_count} watchlists with {entry_count} entries")


# ── Price Alerts ────────────────────────────────────────────────────────

print("\nSeeding price alerts...")

# Buyer alerts: below thresholds
buyer_alerts = [
    ("buyer@buy.com", "VLSFO Conventional", "Singapore", "below", 550),
    ("buyer@buy.com", "VLSFO Conventional", "Rotterdam", "below", 545),
    ("buyer@buy.com", "Methanol Green", "Singapore", "below", 520),
    ("buyer@buy.com", "Methanol Green", "Rotterdam", "below", 560),
    ("buyer@buy.com", "MGO Conventional", "Singapore", "below", 700),
    ("buyer@buy.com", "LNG Conventional", "Singapore", "below", 1000),
]

# Seller alerts: above thresholds
seller_alerts = [
    ("seller@sell.com", "VLSFO Conventional", "ARA", "above", 600),
    ("seller@sell.com", "VLSFO Green", "Rotterdam", "above", 800),
    ("seller@sell.com", "MGO Conventional", "ARA", "above", 750),
    ("seller@sell.com", "MGO Bio", "Rotterdam", "above", 1100),
    ("seller@sell.com", "Methanol Green", "Rotterdam", "above", 620),
]

# Admin alerts: mixed
admin_alerts = [
    ("admin@verdaxis.com", "VLSFO Conventional", "Rotterdam", "below", 500),
    ("admin@verdaxis.com", "Methanol Green", "Singapore", "above", 600),
    ("admin@verdaxis.com", "Ammonia Green", "Houston", "below", 700),
    ("admin@verdaxis.com", "LNG Conventional", "Fujairah", "above", 950),
]

all_alerts = buyer_alerts + seller_alerts + admin_alerts
alert_count = 0

for email, product_name, dp_name, direction, threshold in all_alerts:
    alert_id = str(uuid.uuid4())
    org_id = ORGS[email]
    product_id = PRODUCTS[product_name]
    delivery_point_id = DELIVERY_POINTS[dp_name]

    # 80% active, 20% triggered
    is_active = random.random() < 0.80
    triggered_at = None
    if not is_active and random.random() < 1.0:  # If triggered, add timestamp
        days_ago = random.randint(1, 14)
        triggered_at = datetime.utcnow() - timedelta(days=days_ago)

    now = datetime.utcnow()

    cur.execute(
        """
        INSERT INTO price_alerts
        (id, org_id, product_id, delivery_point_id, direction, threshold_usd, is_active, triggered_at, created_at)
        VALUES (%s, %s, %s, %s, %s, %s, %s, %s, %s)
        """,
        (alert_id, org_id, product_id, delivery_point_id, direction, threshold, is_active, triggered_at, now)
    )
    alert_count += 1

conn.commit()
print(f"  Created {alert_count} price alerts")


# ── Match Suggestions ───────────────────────────────────────────────────

print("\nQuerying open orders from orderbook...")

# Query for BID and ASK orders
cur.execute(
    """
    SELECT id, organization_id, side, product_id, delivery_point_id, price_per_mt_usd
    FROM orderbook_orders
    WHERE status = 'OPEN'
    ORDER BY side, product_id
    """
)

orders = cur.fetchall()
bids = [o for o in orders if o[2] == "BID"]
asks = [o for o in orders if o[2] == "ASK"]

print(f"  Found {len(bids)} BID orders and {len(asks)} ASK orders")

# Pair BIDs with ASKs for match suggestions
matches = []

for bid in bids:
    bid_id, bid_org, bid_side, bid_product, bid_dp, bid_price = bid

    # Find matching ASKs: same product is required, same delivery point is optional
    matching_asks = [
        a for a in asks
        if a[3] == bid_product  # Same product
    ]

    if matching_asks:
        # Pick up to 2 random matching ASKs
        num_matches = min(random.randint(1, 2), len(matching_asks))
        selected_asks = random.sample(matching_asks, num_matches)

        for ask in selected_asks:
            ask_id, ask_org, ask_side, ask_product, ask_dp, ask_price = ask

            # Calculate score
            score = 0
            reasons = []

            # Same product: +30
            if bid_product == ask_product:
                score += 30
                reasons.append("fuel_type_match")

            # Same delivery point: +25
            if bid_dp and ask_dp and bid_dp == ask_dp:
                score += 25
                reasons.append("region_match")
            elif not bid_dp or not ask_dp:
                score += 10  # Partial credit

            # Price within 5%: +20
            if bid_price and ask_price:
                price_diff = abs(ask_price - bid_price) / ask_price if ask_price > 0 else 0
                if price_diff <= 0.05:
                    score += 20
                    reasons.append("price_proximity")
                elif price_diff <= 0.10:
                    score += 10

            # Random noise: ±10
            noise = random.randint(-10, 10)
            score += noise

            # Clamp score to 0-100
            score = max(0, min(100, score))

            # Ensure at least one reason
            if not reasons:
                reasons.append("market_opportunity")

            # Alternate recipient between bid and ask org
            if random.random() < 0.5:
                recipient_org_id = bid_org
            else:
                recipient_org_id = ask_org

            matches.append({
                "bid_order_id": bid_id,
                "ask_order_id": ask_id,
                "score": score,
                "match_reasons": reasons,
                "recipient_org_id": recipient_org_id,
            })

# Ensure we have at least 40 matches (or use all we have if less)
if len(matches) < 40:
    print(f"  Warning: only {len(matches)} matches found (target: 40)")
else:
    # Trim to 40 if we have more
    matches = random.sample(matches, min(len(matches), 40))

# Insert match suggestions with distributed statuses and timestamps
match_count = 0
status_distribution = ["SUGGESTED"] * 20 + ["VIEWED"] * 12 + ["ACTED"] * 4 + ["DISMISSED"] * 4

for i, match in enumerate(matches[:40]):
    suggestion_id = str(uuid.uuid4())
    status = status_distribution[i % len(status_distribution)]

    # Scatter created_at over last 14 days
    days_ago = random.uniform(0, 14)
    hours_ago = random.uniform(0, 23)
    created_at = datetime.utcnow() - timedelta(days=days_ago, hours=hours_ago)

    cur.execute(
        """
        INSERT INTO match_suggestions
        (id, bid_order_id, ask_order_id, score, match_reasons, status, recipient_org_id, created_at)
        VALUES (%s, %s, %s, %s, %s, %s, %s, %s)
        """,
        (
            suggestion_id,
            match["bid_order_id"],
            match["ask_order_id"],
            match["score"],
            json.dumps(match["match_reasons"]),
            status,
            match["recipient_org_id"],
            created_at,
        )
    )
    match_count += 1

conn.commit()
print(f"  Created {match_count} match suggestions")


# ── Summary ──────────────────────────────────────────────────────────────

print("\n== Seed Summary ==")

cur.execute("SELECT COUNT(*) FROM watchlists")
wl_total = cur.fetchone()[0]
print(f"  Watchlists: {wl_total}")

cur.execute("SELECT COUNT(*) FROM watchlist_entries")
wl_entries_total = cur.fetchone()[0]
print(f"  Watchlist entries: {wl_entries_total}")

cur.execute("SELECT COUNT(*) FROM price_alerts")
pa_total = cur.fetchone()[0]
print(f"  Price alerts: {pa_total}")

cur.execute("SELECT COUNT(*) FROM price_alerts WHERE is_active = true")
pa_active = cur.fetchone()[0]
print(f"    Active: {pa_active}")

cur.execute("SELECT COUNT(*) FROM price_alerts WHERE triggered_at IS NOT NULL")
pa_triggered = cur.fetchone()[0]
print(f"    Triggered: {pa_triggered}")

cur.execute("SELECT COUNT(*) FROM match_suggestions")
ms_total = cur.fetchone()[0]
print(f"  Match suggestions: {ms_total}")

cur.execute(
    """
    SELECT status, COUNT(*) FROM match_suggestions GROUP BY status ORDER BY status
    """
)
for status, count in cur.fetchall():
    print(f"    {status}: {count}")

cur.close()
conn.commit()
conn.close()

print("\nDone! User activity data seeded successfully.")
