# Market Signal Ingestion Runbook

Operator guide for turning external market data (broker sheets, desk marks,
partner CSVs) into **verified real** Forward Curve monitoring signals via
`scripts/ingest_market_signals.py`. One file = one signal family = one source
= one ingestion run.

## Trust model (why this exists)

A signal row renders as REAL on the public Forward Curve only when the trust
predicate in `app/services/forward_monitoring.py` passes: the row is non-demo,
`is_verified_real`, points at an ingestion run whose `verified_at` is set, and
the run's `source`, `signal_family`, and `source_kind` all match. The importer
is the only sanctioned way to produce such rows.

| CLI family | Run `signal_family` | Run `source_kind` |
|---|---|---|
| `indications` | `MARKET_INDICATION` | `MARKET_INDICATION` |
| `fair_bands` | `FAIR_PRICE_BAND` | **`FAIR_PRICE_MODEL`** (the enums differ!) |
| `stems` | `PHYSICAL_STEM` | `PHYSICAL_STEM` |

`FAMILY_TO_SOURCE_KIND` in `app/services/market_signal_ingestion.py` is the
single source of truth; a wrong `source_kind` passes the DB CHECK but fails the
trust join, silently rendering every row untrusted.

## CSV formats (exact headers)

**indications.csv**
```
source_event_id,market_product,delivery_point,availability_window,side,price_per_mt_usd,quantity_mt,observed_at
```

**fair_bands.csv**
```
source_event_id,market_product,delivery_point,availability_window,low_price_per_mt_usd,mid_price_per_mt_usd,high_price_per_mt_usd,model_name,model_version,observed_at
```

**stems.csv**
```
source_event_id,stem_uid,market_product,delivery_point,availability_window,status,quantity_mt,stem_start,stem_end,observed_at
```

Rules:
- `source` is a CLI argument (`^[a-z0-9][a-z0-9_-]{1,63}$`), not a CSV column.
- `source_event_id` is REQUIRED in every row: it drives idempotency through the
  partial unique `(source, source_event_id)` indexes. Re-ingesting a file skips
  duplicates; it never double-inserts.
- `market_product` must be one of the canonical four (`BIO_METHANOL`,
  `E_METHANOL`, `BIO_ETHANOL`, `SYNTHETIC_ETHANOL`).
- `delivery_point` is an exact active catalog name (e.g. `Singapore`) or the
  delivery-point UUID.
- `availability_window` must normalize through
  `app/services/availability_windows.py`: `SPOT`, `YYYY-MM`, `YYYY-QN`,
  `YYYY-CAL`. UI-relative labels like `M+1` are rejected.
- `observed_at` must be ISO-8601 **with timezone**; naive or >5min-future
  timestamps are rejected.
- Prices are parsed as `Decimal` and quantized to 2dp; `side` ∈ BID/ASK/MID;
  stem `status` ∈ AVAILABLE/TENTATIVE/ALLOCATED/CANCELLED.
- Stems are append-only latest-state per `(source, stem_uid)`: an update is a
  NEW row with the same `stem_uid` and a **strictly later** `observed_at`
  (corrections re-imported with an equal timestamp lose to `created_at, id`
  ordering). Never mint a fresh `stem_uid` per file — that double-counts
  availability.

## Procedure (dry-run first, always)

```bash
cd /home/verdaxis-prod/verdaxis/staging/be

# 1. Dry run: full validation report, zero writes
sudo -u verdaxis-prod ./venv/bin/python scripts/ingest_market_signals.py \
  --family indications --source desk-marks --file /path/to/indications.csv

# 2. Fix any row errors (reported with row numbers), re-run dry

# 3. Commit
sudo -u verdaxis-prod ./venv/bin/python scripts/ingest_market_signals.py \
  --family indications --source desk-marks --file /path/to/indications.csv --commit

# 4. Record the printed run id in the ops log
```

Guards: the CLI refuses to run outside the staging backend tree or against a
database not named `verdaxis_staging`. `--allow-db` loosens the name check for
a future production decision only — **using it against prod requires Jon's
explicit sign-off**, and the prod-promotion slice (roadmap H0.1) must land
first.

## Staleness ≠ rejection

Rows older than the board lookbacks (7d indications / 30d fair bands / 90d
stems) import successfully but never render — the report counts them as
`stale warnings` so the operator knows why the board looks unchanged instead of
concluding the importer is broken.

## Redaction invariant (load-bearing)

Public slice/table JSON never exposes raw `source`, `source_record_id`,
`source_event_id`, `stem_uid`, `model_name`, `model_version`, or ingestion run
ids. Never add such fields to `app/schemas/curves.py`. Verify after any
ingestion-adjacent change:

```bash
curl -s 'https://api-staging.verdaxis.exchange/api/curves/forward/slice?market_product=BIO_METHANOL&delivery_point_id=<UUID>&availability_window=SPOT' \
  | grep -c '<your-source-name>\|<a-source-event-id>'   # must print 0
```

## Rolling back a bad run

Every row carries `trusted_ingestion_run_id`. Delete the rows, then the run:

```sql
DELETE FROM market_indications  WHERE trusted_ingestion_run_id = '<run-id>';
-- or fair_price_bands / physical_stems for those families
DELETE FROM market_signal_ingestion_runs WHERE id = '<run-id>';
```

A run whose rows all failed or duplicated is never committed by the service
(zero-inserted runs are deleted before flush) — a verified run with zero rows
would be a forgery primitive.

## Verified end-to-end (2026-07-10)

Live staging round-trip: 2-row indications CSV for BIO_METHanol/Singapore/SPOT
ingested (run `94b30383…`, since deleted); public slice API returned the
indication summary with `demo_status: REAL_ONLY`; response contained zero
source/event identifiers; Forward Curve drilldown rendered both rows as amber
IND markers labeled "Market indication" (not "Demo data"). Unit coverage:
`tests/unit/test_market_signal_ingestion.py` (14 tests incl. the trust
round-trip for all three families).
