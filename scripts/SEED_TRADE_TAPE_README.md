# Trade Tape Seed Script

## Purpose

`seed_trade_tape.py` generates a rich, realistic trade history for the Verdaxis maritime fuel trading platform. It creates 300+ trades spanning January 1 - March 29, 2026, with realistic:
- Product mix and pricing (8 fuel types with 2026 market rates)
- Order quantities and trade volumes
- Trade lifecycle (PENDING_CONFIRMATION through PAID)
- Commission tracking and invoicing
- Market price drift (3-5% upward trend over 3 months)
- Weekday clustering (3-5 trades/weekday, 0-2 on weekends)

## Requirements

- Python 3.7+
- `psycopg2` library
- PostgreSQL running with verdaxis database
- Credentials in DB_CONFIG (currently set for localhost:5432)

## Usage

```bash
cd /home/verdaxis-prod/verdaxis-backend
python3 scripts/seed_trade_tape.py
```

## What It Does

### 1. Cleans Existing Seeded Data (Idempotent)
- Deletes commissions where trade_id references Jan 2026+ trades without order linkage
- Deletes the associated trades
- Safe to run multiple times

### 2. Generates Trade Dates
- 300 dates distributed across 89 days (Jan 1 - Mar 29, 2026)
- Weekday clustering: more trades on Mon-Fri, fewer on weekends
- Random times within business hours (1:00-16:59)

### 3. Creates Trade Records

Each trade includes:
- **Identifiers**: UUID, null bid_order_id/ask_order_id (historical trades)
- **Parties**: Random buyer & seller from provided org lists
- **Product**: 8 fuel types with weighted distribution
  - VLSFO: 35% (520-620 $/MT, 200-8000 MT)
  - MGO: 15% (650-780 $/MT)
  - Methanol Green: 15% (480-700 $/MT)
  - LNG: 10% (800-1200 $/MT)
  - Biofuel: 10% (900-1300 $/MT)
  - Ammonia: 8% (600-900 $/MT)
  - VLSFO Green: 5% (700-850 $/MT)
  - MGO Bio: 2% (950-1200 $/MT)

- **Pricing**: With 3-5% drift upward over the period
- **Quantities**: Product-specific ranges (VLSFO 200-8000 MT, LNG 2000-10000 MT, etc.)
- **Status Distribution**:
  - PAID: 45% (oldest trades, fully settled)
  - DELIVERED: 20%
  - CONFIRMED: 15%
  - PENDING_CONFIRMATION: 10%
  - CANCELLED: 7%
  - DECLINED: 3%

- **Timestamps**:
  - created_at: Trade date
  - confirmed_at: +1-8 hours (for CONFIRMED and later)
  - delivered_at: confirmed_at + 3-14 days (for DELIVERED and later)
  - paid_at: delivered_at + 5-30 days (for PAID)

- **Final Quantities/Prices** (for PAID/DELIVERED):
  - Final quantity: ±5% variance from ordered quantity
  - Final price: ±2% variance from negotiated price
  - Final total: final_qty * final_price

- **Commission**:
  - Rate: 0.5%
  - Amount: price_per_mt * quantity * 0.005
  - Commission records created for PAID/DELIVERED/CONFIRMED trades

- **Other Fields**:
  - is_anonymous: 70% true
  - initiated_by: 50% BUYER, 50% SELLER

### 4. Creates Commission Records

For each trade with status PAID/DELIVERED/CONFIRMED:
- **PAID trades**: commission status = 'PAID', invoice_number = 'VDX-2026-XXXXXX', invoice_date = payment_date
- **DELIVERED**: commission status = 'INVOICED', invoice_number generated, invoice_date = delivery + 2 days
- **CONFIRMED**: commission status = 'PENDING', no invoice yet

## Output

Script prints a detailed summary:

```
Verdaxis Trade Tape Seed Script
======================================================================

Target period: 2026-01-01 to 2026-03-29
Target trade count: 300

Cleaning existing seeded trades...
Cleaned 0 trades and 0 commissions

Generating 300 trade dates...
Generated 300 dates

Generating 300 trades...
  Generated 50/300 trades
  Generated 100/300 trades
  ...
  Generated 300/300 trades

Inserting trades and commissions...
Inserted 300 trades
Inserted 230 commissions

======================================================================
TRADE TAPE SEED SUMMARY
======================================================================

Total Trades Generated: 300

Status Distribution:
  CANCELLED          :  21 (  7.0%)
  CONFIRMED          :  45 ( 15.0%)
  DECLINED           :   9 (  3.0%)
  DELIVERED          :  60 ( 20.0%)
  PAID               : 135 ( 45.0%)
  PENDING_CONFIRMATION :  30 ( 10.0%)

Product Distribution:
  Ammonia            :  24 (  8.0%)
  Biofuel            :  30 ( 10.0%)
  LNG                :  30 ( 10.0%)
  Methanol Green     :  45 ( 15.0%)
  MGO                :  45 ( 15.0%)
  MGO Bio            :   6 (  2.0%)
  VLSFO              : 105 ( 35.0%)
  VLSFO Green        :  15 (  5.0%)

Volume & Value:
  Total Quantity: 1,234,567 MT
  Total Trade Value: $750,123,456.78
  Total Commissions: $3,750,617.28

Initiator Split:
  BUYER:  150 ( 50.0%)
  SELLER: 150 ( 50.0%)

Anonymity:
  Anonymous: 210 ( 70.0%)

======================================================================

Success! Seeded 300 trades and 230 commissions.
```

## Database Tables Modified

### trades
- id (UUID primary key)
- bid_order_id, ask_order_id (NULL for seeded trades)
- buyer_id, seller_id (FK to organizations)
- quantity_mt, price_per_mt_usd
- final_quantity_mt, final_price_per_mt, final_total_usd (for PAID/DELIVERED)
- status, initiated_by, is_anonymous
- commission_rate_pct, commission_amount_usd
- confirmed_at, delivered_at, paid_at
- created_at

### commissions
- id (UUID primary key)
- trade_id (FK to trades)
- amount_usd
- status (PENDING/INVOICED/PAID)
- invoice_number, invoice_date, payment_date
- notes
- created_at, updated_at

## Idempotency

The script is fully idempotent. Running it multiple times:
1. Deletes all previous seeded trades (Jan 2026+ with null order IDs)
2. Generates fresh synthetic data with different random values
3. Produces consistent summary statistics

This makes it safe for development/testing iterations.

## Configuration

Edit these constants in the script to customize:

- `DB_CONFIG`: Database connection parameters
- `BUYERS`: Buyer organization UUIDs
- `SELLERS`: Supplier organization UUIDs
- `PRODUCTS`: Fuel types, price ranges, quantity ranges
- `STATUS_DISTRIBUTION`: % breakdown by trade status
- `COMMISSION_RATE`: Currently 0.5%
- In `main()`: `num_trades` (currently 300)

## Troubleshooting

### Connection Error
```
psycopg2.OperationalError: could not connect to server
```
Check:
- PostgreSQL is running: `sudo systemctl status postgresql`
- Database exists: `psql -l | grep verdaxis`
- Credentials in DB_CONFIG match your environment

### Permission Error
```
Permission denied: '/home/verdaxis-prod/verdaxis-backend/scripts/seed_trade_tape.py'
```
Ensure file is executable:
```bash
sudo chmod +x /home/verdaxis-prod/verdaxis-backend/scripts/seed_trade_tape.py
```

### Missing Module
```
ModuleNotFoundError: No module named 'psycopg2'
```
Install dependencies:
```bash
pip install psycopg2-binary
# or
cd /home/verdaxis-prod/verdaxis-backend
pip install -r requirements.txt
```

### Data Not Appearing
Check if trades are for the right date range:
```sql
SELECT COUNT(*) FROM trades WHERE created_at >= '2026-01-01';
```

## Notes

- The script uses `conn.autocommit = False` with explicit `commit()` calls for transaction control
- All monetary amounts are Decimal type for precision
- Timestamps are UTC (via `datetime.now()`)
- Commission invoice numbers are VDX-2026-XXXXXX format (where XXXXXX is a 6-char hex string)
- Price drift is calculated per trade based on its date within the 89-day period
