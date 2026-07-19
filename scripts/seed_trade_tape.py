#!/usr/bin/env python3
"""
Seed script for Verdaxis trade tape.
Generates 300+ realistic trades spanning Jan 1 - Mar 29, 2026.
Includes lifecycle status progression, commission tracking, and market price drift.
"""

import psycopg2
from datetime import datetime, timedelta
import uuid
import random
from decimal import Decimal
import sys
from app.seeds.safety import seed_connection

# Organization IDs
BUYERS = [
    'acc3f20a-fe94-4463-9029-a55e35634eb7',  # Buy Corp
    '4da7b285-34ee-5443-9406-f96b4ed1a251',  # Maersk
    '0dbce576-2026-5925-ab66-674d505e98ad',  # Evergreen
    '3cd0bc8a-0e92-5eb8-9ae3-3c87a7460a6a',  # COSCO
    '277491df-cb0d-5f2d-a2cf-5746829c6da6',  # MSC
    '3b302066-d65c-5c3e-8fcc-70b3da3bcafd',  # CMA CGM
]

SELLERS = [
    'c9c1ccbf-66fe-4a1b-b171-fe4f7ddc31a4',  # Sell Corp
    '79609f48-0a3e-560e-a1e1-63d90601d84a',  # Vitol
    '2c4e387e-de22-5adb-ad88-9274ba84ebe1',  # Trafigura
    '612953c7-567a-58b3-bc42-ee817d2bbe74',  # OCI
    '93ccda09-54b3-53ee-afc0-759d3048161f',  # Peninsula
    '82426590-0963-5486-9b05-f81e97afe6ef',  # Bunker Holding
]

# Product specs: (name, price_range_min, price_range_max, quantity_min, quantity_max, weight)
PRODUCTS = {
    'VLSFO': (520, 620, 200, 8000, 0.35),
    'MGO': (650, 780, 200, 8000, 0.15),
    'Methanol Green': (480, 700, 500, 5000, 0.15),
    'LNG': (800, 1200, 2000, 10000, 0.10),
    'Biofuel': (900, 1300, 500, 5000, 0.10),
    'Ammonia': (600, 900, 500, 5000, 0.08),
    'VLSFO Green': (700, 850, 500, 5000, 0.05),
    'MGO Bio': (950, 1200, 500, 5000, 0.02),
}

# Status distribution
STATUS_DISTRIBUTION = {
    'PAID': 0.45,
    'DELIVERED': 0.20,
    'CONFIRMED': 0.15,
    'PENDING_CONFIRMATION': 0.10,
    'CANCELLED': 0.07,
    'DECLINED': 0.03,
}

COMMISSION_RATE = 0.005  # 0.5%

def get_connection():
    """Create database connection."""
    return seed_connection()

def clean_existing_trades(conn):
    """Remove existing seeded trades (idempotent)."""
    cur = conn.cursor()
    try:
        # Delete commissions for trades we're about to delete
        cur.execute("""
            DELETE FROM commissions
            WHERE trade_id IN (
                SELECT id FROM trades
                WHERE created_at >= '2026-01-01'
                AND bid_order_id IS NULL
                AND ask_order_id IS NULL
            )
        """)
        deleted_commissions = cur.rowcount

        # Delete the trades
        cur.execute("""
            DELETE FROM trades
            WHERE created_at >= '2026-01-01'
            AND bid_order_id IS NULL
            AND ask_order_id IS NULL
        """)
        deleted_trades = cur.rowcount

        conn.commit()
        print(f"Cleaned {deleted_trades} trades and {deleted_commissions} commissions")
        return deleted_trades, deleted_commissions
    except Exception as e:
        conn.rollback()
        print(f"Error cleaning: {e}")
        raise

def generate_trade_dates(num_trades, start_date, end_date):
    """
    Generate trade dates distributed across the period.
    Cluster trades on weekdays (Mon-Fri), fewer on weekends.
    """
    dates = []
    current = start_date

    while current <= end_date:
        # Weekday: 3-5 trades, Weekend: 0-2 trades
        weekday = current.weekday()  # 0=Mon, 6=Sun
        if weekday < 5:  # Mon-Fri
            num_trades_today = random.randint(3, 5)
        else:  # Sat-Sun
            num_trades_today = random.randint(0, 2)

        for _ in range(num_trades_today):
            hour = random.randint(1, 16)
            minute = random.randint(0, 59)
            second = random.randint(0, 59)
            dt = current.replace(hour=hour, minute=minute, second=second)
            dates.append(dt)

        current += timedelta(days=1)

    # Shuffle and trim to exact count
    random.shuffle(dates)
    return sorted(dates[:num_trades])

def get_status_for_date(trade_date, end_date):
    """
    Assign status based on how old the trade is.
    Older trades are more likely to be PAID/DELIVERED.
    """
    days_ago = (end_date - trade_date).days

    if days_ago < 1:
        # Very recent: PENDING_CONFIRMATION
        return 'PENDING_CONFIRMATION'
    elif days_ago < 5:
        # Recent: PENDING_CONFIRMATION or CONFIRMED
        return random.choices(['PENDING_CONFIRMATION', 'CONFIRMED'],
                            weights=[0.4, 0.6])[0]
    elif days_ago < 14:
        # Mid-recent: CONFIRMED or DELIVERED
        return random.choices(['CONFIRMED', 'DELIVERED'],
                            weights=[0.3, 0.7])[0]
    else:
        # Old: any status, weighted by distribution
        return random.choices(
            list(STATUS_DISTRIBUTION.keys()),
            weights=list(STATUS_DISTRIBUTION.values())
        )[0]

def get_price_with_drift(product_name, min_price, max_price, trade_date, start_date, end_date):
    """
    Generate price with market drift.
    Prices trend up 3-5% over the 3-month period.
    """
    # Base price
    base_price = Decimal(str(random.uniform(min_price, max_price))).quantize(Decimal('0.01'))

    # Price drift: later trades slightly more expensive
    days_elapsed = (trade_date - start_date).days
    total_days = (end_date - start_date).days
    progress = days_elapsed / total_days if total_days > 0 else 0
    drift_factor = 1.0 + (random.uniform(0.03, 0.05) * progress)

    drifted_price = Decimal(str(float(base_price) * drift_factor)).quantize(Decimal('0.01'))
    return drifted_price

def select_product():
    """Select a product based on weighted distribution."""
    products = list(PRODUCTS.keys())
    weights = [PRODUCTS[p][4] for p in products]
    return random.choices(products, weights=weights)[0]

def generate_trade(trade_date, end_date, trade_id):
    """Generate a single trade record."""
    # Select product and generate price
    product = select_product()
    min_price, max_price, qty_min, qty_max, _ = PRODUCTS[product]

    # Price with drift
    start_date = datetime(2026, 1, 1)
    price_per_mt = get_price_with_drift(product, min_price, max_price, trade_date, start_date, end_date)

    # Quantity
    quantity_mt = Decimal(str(random.randint(qty_min, qty_max)))

    # Organizations
    buyer_id = random.choice(BUYERS)
    seller_id = random.choice(SELLERS)

    # Status and lifecycle
    status = get_status_for_date(trade_date, end_date)

    # Confirmation and delivery timestamps
    confirmed_at = None
    delivered_at = None
    paid_at = None

    if status in ['CONFIRMED', 'DELIVERED', 'PAID']:
        confirmed_at = trade_date + timedelta(hours=random.randint(1, 8))

    if status in ['DELIVERED', 'PAID']:
        delivered_at = confirmed_at + timedelta(days=random.randint(3, 14))

    if status == 'PAID':
        paid_at = delivered_at + timedelta(days=random.randint(5, 30))

    # Final quantities and prices (for PAID/DELIVERED trades, with ±5% variance)
    final_quantity_mt = None
    final_price_per_mt = None
    final_total_usd = None

    if status in ['DELIVERED', 'PAID']:
        variance = Decimal(str(random.uniform(0.95, 1.05)))
        final_quantity_mt = (quantity_mt * variance).quantize(Decimal('0.01'))

        price_variance = Decimal(str(random.uniform(0.98, 1.02)))
        final_price_per_mt = (price_per_mt * price_variance).quantize(Decimal('0.01'))

        final_total_usd = (final_quantity_mt * final_price_per_mt).quantize(Decimal('0.01'))

    # Commission
    commission_amount_usd = (price_per_mt * quantity_mt * Decimal(str(COMMISSION_RATE))).quantize(Decimal('0.01'))

    # Other fields
    initiated_by = random.choice(['BUYER', 'SELLER'])
    is_anonymous = random.random() < 0.70  # 70% anonymous

    return {
        'id': trade_id,
        'bid_order_id': None,
        'ask_order_id': None,
        'buyer_id': buyer_id,
        'seller_id': seller_id,
        'initiated_by': initiated_by,
        'is_anonymous': is_anonymous,
        'quantity_mt': quantity_mt,
        'price_per_mt_usd': price_per_mt,
        'status': status,
        'final_quantity_mt': final_quantity_mt,
        'final_price_per_mt': final_price_per_mt,
        'final_total_usd': final_total_usd,
        'commission_rate_pct': Decimal('0.5'),
        'commission_amount_usd': commission_amount_usd,
        'confirmed_at': confirmed_at,
        'delivered_at': delivered_at,
        'paid_at': paid_at,
        'created_at': trade_date,
        'product': product,
    }

def insert_trades(conn, trades):
    """Insert all trades and associated commissions."""
    cur = conn.cursor()

    try:
        # Insert trades
        for trade in trades:
            cur.execute("""
                INSERT INTO trades (
                    id, bid_order_id, ask_order_id, buyer_id, seller_id,
                    initiated_by, is_anonymous, quantity_mt, price_per_mt_usd,
                    status, final_quantity_mt, final_price_per_mt, final_total_usd,
                    commission_rate_pct, commission_amount_usd,
                    confirmed_at, delivered_at, paid_at, created_at
                ) VALUES (
                    %s, %s, %s, %s, %s,
                    %s, %s, %s, %s,
                    %s, %s, %s, %s,
                    %s, %s,
                    %s, %s, %s, %s
                )
            """, (
                trade['id'], trade['bid_order_id'], trade['ask_order_id'],
                trade['buyer_id'], trade['seller_id'],
                trade['initiated_by'], trade['is_anonymous'],
                trade['quantity_mt'], trade['price_per_mt_usd'],
                trade['status'], trade['final_quantity_mt'],
                trade['final_price_per_mt'], trade['final_total_usd'],
                trade['commission_rate_pct'], trade['commission_amount_usd'],
                trade['confirmed_at'], trade['delivered_at'],
                trade['paid_at'], trade['created_at']
            ))

        conn.commit()
        print(f"Inserted {len(trades)} trades")

        # Insert commissions for PAID/DELIVERED/CONFIRMED trades
        commission_count = 0
        for trade in trades:
            if trade['status'] in ['PAID', 'DELIVERED', 'CONFIRMED']:
                commission_id = uuid.uuid4()

                if trade['status'] == 'PAID':
                    comm_status = 'PAID'
                    invoice_number = f"VDX-2026-{uuid.uuid4().hex[:6].upper()}"
                    invoice_date = trade['paid_at'].date() if trade['paid_at'] else None
                    payment_date = trade['paid_at'].date() if trade['paid_at'] else None
                elif trade['status'] == 'DELIVERED':
                    comm_status = 'INVOICED'
                    invoice_number = f"VDX-2026-{uuid.uuid4().hex[:6].upper()}"
                    invoice_date = (trade['delivered_at'] + timedelta(days=2)).date() if trade['delivered_at'] else None
                    payment_date = None
                else:  # CONFIRMED
                    comm_status = 'PENDING'
                    invoice_number = None
                    invoice_date = None
                    payment_date = None

                cur.execute("""
                    INSERT INTO commissions (
                        id, match_id, trade_id, amount_usd, status,
                        invoice_number, invoice_date, payment_date,
                        notes, created_at, updated_at
                    ) VALUES (
                        %s, %s, %s, %s, %s,
                        %s, %s, %s,
                        %s, %s, %s
                    )
                """, (
                    commission_id, None, trade['id'],
                    trade['commission_amount_usd'], comm_status,
                    invoice_number, invoice_date, payment_date,
                    f"Auto-generated for {trade['product']}",
                    datetime.now(), datetime.now()
                ))
                commission_count += 1

        conn.commit()
        print(f"Inserted {commission_count} commissions")

        return len(trades), commission_count

    except Exception as e:
        conn.rollback()
        print(f"Error inserting trades: {e}")
        raise

def print_summary(trades):
    """Print summary statistics."""
    print("\n" + "="*70)
    print("TRADE TAPE SEED SUMMARY")
    print("="*70)

    print(f"\nTotal Trades Generated: {len(trades)}")

    # By status
    status_counts = {}
    for trade in trades:
        status = trade['status']
        status_counts[status] = status_counts.get(status, 0) + 1

    print("\nStatus Distribution:")
    for status, count in sorted(status_counts.items()):
        pct = (count / len(trades)) * 100
        print(f"  {status:20s}: {count:3d} ({pct:5.1f}%)")

    # By product
    product_counts = {}
    for trade in trades:
        product = trade['product']
        product_counts[product] = product_counts.get(product, 0) + 1

    print("\nProduct Distribution:")
    for product, count in sorted(product_counts.items()):
        pct = (count / len(trades)) * 100
        print(f"  {product:20s}: {count:3d} ({pct:5.1f}%)")

    # Volume and value
    total_quantity = sum(Decimal(str(t['quantity_mt'])) for t in trades)
    total_value = sum(Decimal(str(t['quantity_mt'])) * Decimal(str(t['price_per_mt_usd'])) for t in trades)
    total_commissions = sum(Decimal(str(t['commission_amount_usd'])) for t in trades)

    print(f"\nVolume & Value:")
    print(f"  Total Quantity: {total_quantity:,.0f} MT")
    print(f"  Total Trade Value: ${total_value:,.2f}")
    print(f"  Total Commissions: ${total_commissions:,.2f}")

    # Initiated by
    buyer_init = sum(1 for t in trades if t['initiated_by'] == 'BUYER')
    seller_init = sum(1 for t in trades if t['initiated_by'] == 'SELLER')

    print(f"\nInitiator Split:")
    print(f"  BUYER:  {buyer_init} ({(buyer_init/len(trades)*100):5.1f}%)")
    print(f"  SELLER: {seller_init} ({(seller_init/len(trades)*100):5.1f}%)")

    # Anonymous
    anon_count = sum(1 for t in trades if t['is_anonymous'])
    print(f"\nAnonymity:")
    print(f"  Anonymous: {anon_count} ({(anon_count/len(trades)*100):5.1f}%)")

    print("\n" + "="*70)

def main():
    """Main execution."""
    print("Verdaxis Trade Tape Seed Script")
    print("="*70)

    start_date = datetime(2026, 1, 1)
    end_date = datetime(2026, 3, 29)
    num_trades = 300

    print(f"Target period: {start_date.date()} to {end_date.date()}")
    print(f"Target trade count: {num_trades}")

    # Connect
    conn = get_connection()
    conn.autocommit = False

    try:
        # Clean existing
        print("\nCleaning existing seeded trades...")
        clean_existing_trades(conn)

        # Generate trade dates
        print(f"\nGenerating {num_trades} trade dates...")
        trade_dates = generate_trade_dates(num_trades, start_date, end_date)
        print(f"Generated {len(trade_dates)} dates")

        # Generate trades
        print(f"\nGenerating {len(trade_dates)} trades...")
        trades = []
        for i, trade_date in enumerate(trade_dates):
            trade = generate_trade(trade_date, end_date, uuid.uuid4())
            trades.append(trade)
            if (i + 1) % 50 == 0:
                print(f"  Generated {i + 1}/{len(trade_dates)} trades")

        # Insert trades and commissions
        print(f"\nInserting trades and commissions...")
        trade_count, commission_count = insert_trades(conn, trades)

        # Print summary
        print_summary(trades)

        print(f"\nSuccess! Seeded {trade_count} trades and {commission_count} commissions.")

    except Exception as e:
        print(f"\nFailed: {e}")
        sys.exit(1)
    finally:
        conn.close()

if __name__ == '__main__':
    main()
