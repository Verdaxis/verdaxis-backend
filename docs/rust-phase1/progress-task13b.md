# Task 13b — Phase 0 convergence: B30/B100 policy, biofuel clause, re-recorded corpus

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`. Base: merge
`95ba1b36` (Phase 0 convergence `origin/staging` `56d75770` into the Rust
read-service branch). No live access, push, agents, subagents or skills.
Every build and test ran through `verdaxis-prod-test "$PWD" '…'`.

## What Phase 0 changed and what this task had to follow

The merge made the Python tree the production oracle: `products` gained the two
canonical biofuel rows (B30 `c4cecebc-…`, B100 `74eb9cb5-…`), the reviewed
`PROFILES` map in `scripts/rust_contract/export_policy.py` had no entry for
them, `app/services/execution_policy.py` added
`biofuel_supplier_metadata_clause()` and applied it to public ASK order and
inventory projections, and `APPROVED_MARKETPLACE_FUEL_TYPES` gained
`"Biofuel"`. The committed policy artifacts and the golden corpus still listed
six products and were recorded at oracle `f7b6c7b8` (before Phase 0).

## Step 1 — B30/B100 profiles and regenerated policy (commit d6ca6ed5)

`PROFILES` gains `"B30": "scalar"` and `"B100": "scalar"`. This is the Python
oracle's behaviour, not a design choice:

- Neither id is in `is_fame_product()` (`app/services/fame_order.py:22`) or
  `is_lng_product()` (`app/services/lng_order.py:27`), so an order carries no
  `fame_terms`/`lng_terms` requirement and no typed declaration-currency rule
  applies.
- `order_is_execution_qualified` reaches both only through the generic scalar
  ASK path (certification scheme and declaration) plus the data-driven
  `BIOFUEL_SPECIFICATION_STANDARDS` metadata check
  (`app/services/execution_policy.py:205-216`). A fixed per-product contract
  enforced from a table is not a new rule version, so the profile stays
  `scalar`.
- `available_delivery_point_ids` is `None` for both, so delivery is
  unrestricted, matching the other scalar fuels.

Regenerated from the still-current `f7b6c7b8` oracle (step 3 re-records):
`deploy/rust/market-policy.json` (8 products, display order 1–8 in
`CANONICAL_PRODUCTS` order), `deploy/rust/mcp-catalog.json` (B30/B100
`list_markets` entries), `market-policy.manifest.json` (digests), and the
`market_product_enum`/`seed_quotes` sections of
`rs/read/src/market/{curve_policy,orderbook_query}.json`. B30/B100 seed quotes
come from `app.services.benchmarks._base_benchmark_price` over
`app.seeds.market_seed.PRICING` (B30 Singapore 1105.00 / Rotterdam 869.00;
B100 1362.00 everywhere). Catalog vectors stayed byte-identical to the golden
R03/R04 bodies. `test_policy_export` and the offline `policy` target pass;
`export_policy --check` passes.

## Step 2 — mirror the biofuel clause in Rust (commit 4ed68a2f)

Python applies `biofuel_supplier_metadata_clause()` inside
`current_public_order_clause` (`app/services/market_data_eligibility.py:267`)
for the ASK side of every public order collection, and to the public inventory
projection (`:361`) and `/api/availability` (`app/routers/availability.py:116`).
Inventory and `/api/availability` are not ported, so the Rust mirror belongs in
the shared order relation.

- **Policy data, not literals.** `BIOFUEL_SPECIFICATION_STANDARDS` travels as
  exported policy data: `market-policy.json` carries `specification_standard`
  for fixed-biofuel products only (the key is omitted for the others, matching
  the loader's `skip_serializing_if` shape). `ProductPolicy` gains the optional
  field, and both the exporter and `validate_policy` refuse a blank standard.
- **The SQL mirror** in `rs/read/sql/current_order_rule.sql` is byte-equivalent
  to the oracle's rendered clause: `upper(trim(o.specification_standard,
  E' \t\r\n\f\v')) = upper(pb.specification_standard)` (the escape set is
  Python `str.strip`'s default), plus non-empty `carbon_intensity_method`,
  `certification_scheme`, `feedstock`, `origin`, `certification_declared`,
  `msds_available`, and non-null `carbon_intensity_gco2_mj`, gated on
  `o.side <> 'ASK'` and `o.off_spec IS false`. The biofuel products are found
  through `policy_products.specification_standard IS NOT NULL`, so no product
  code or UUID appears in SQL. The policy-product CTEs in
  `public_orders.sql`, `legacy_curve_orders.sql`, `curve_unknown_metadata.sql`,
  `catalog_products.sql` and `formal_trade_snapshots.sql` expose the column.
- **The pure mirror** in `rs/read/src/market/terms.rs`:
  `order_is_execution_qualified_at` gains the ASK `BIOFUEL_SPECIFICATION_STANDARDS`
  branch (`supplier_product_metadata_is_valid`), matching
  `supplier_product_metadata_error` plus the msds/CI/feedstock/origin presence
  checks. `QualificationInput` carries the five new fields;
  `carbon_intensity_gco2_mj` is presence-only, as in Python (`is None`, SQL
  `IS NOT NULL`).
- `public_orders.sql` and `ADMITTED_FUEL_TYPES` admit `"Biofuel"`.
- **Corpus fixtures**: seven B30/B100 orders — two fully declared ASKs, three
  negatives (wrong standard, missing CI method, missing feedstock) and two BIDs
  — with `market_orders` 46 → 53, a new `fixed_biofuel_supplier_metadata`
  fixture family, and B30/B100 lane-filter cases. The Python matrix comparison
  proves the mirror end to end (oracle selected 18 of 53 orders).
- **Differential proof**:
  `biofuel_supplier_metadata_clause_hides_only_failing_asks` freezes the
  pre-Phase-0 rule (`rs/read/tests/support/current_order_rule_pre_biofuel.sql`)
  and requires the clause to remove exactly the two clause-only negative ASK
  rows, add none, and keep all four positive biofuel rows visible. The
  missing-feedstock arm is already covered by the generic ASK predicates, so it
  cannot newly drop; its exclusion is proven through the Python matrix instead.
  The decision-25 fixture (`current_order_rule_legacy_owner.sql`) gains the same
  clause so that differential still isolates exactly one change.
- `deploy/rust/mcp-http-vectors.json` regenerated (`list_markets` now lists 8
  products); the Phase 0 unit test
  `test_biofuel_public_surfaces_and_live_benchmark_share_fixed_contract` is
  mapped in the assertion ledger (owner 13b).

## Step 3 — re-record the golden corpus (commits c2941f36, d2babcbe)

Re-recorded at this tree's HEAD (`4ed68a2f`) with the documented commands on the
test host: `run_disposable --group all --record-all` for the REST groups in both
KYC states (`TOTAL_UNSTABLE=0; RECORD_FAILURES=0` each) and `record_mcp` for the
MCP group (143 cases each, stable across two passes). One directory,
`golden/4ed68a2f…`, holds 974 case folders (487 per state) with the same eleven
groups and case ids as `f7b6c7b8` — no case was added or removed.
`golden/CURRENT` names the active oracle; `recorded_source_sha` reads it, and
`test_policy_export` asserts the pointer and directory agree. The superseded
`f7b6c7b8` corpus stays in place until the PM approves its removal.

Old-versus-new diff (974 cases; every changed case classified):

- **Catalog enumeration (44 cases, both states)** — the eight-product catalog
  reaches `/api/catalog/products` (R03), the MCP `list_markets` envelope
  (M04/M07/M18), curve table/board products/signals/cells
  (R09/R10b/R11/R11b), product-counts (R20), map summary/compact (R21/R22),
  distinct products and fuel types (R23/R25), the price enum detail (R12) and
  the enum-validation messages (R10b/R15b). Each adds B30/B100 in catalog
  order; the pre-existing items are byte-identical (verified as multisets, zero
  old items lost).
- **Product-count and fixture-driven totals** — R16-bids total 10023 → 10025,
  R17-asks total 22 → 24, R19-aggregated 12 → 16, R27-listings 23 → 25,
  T4B-region-* 18 → 20, LB-all/LB-expired 288 → 384 cells — all from the seven
  new B30/B100 corpus rows.
- **Fixed-page shifts from those rows** — LB-sparse lost 42 cells that all
  appear unchanged in the new LB-all; R17-asks page 1 lost two ASK items
  (`040dbf52…`, `010b6ad9…`) that still appear unchanged in R27-listings and the
  region cases. Both are limit/page-boundary effects, not content changes.
- **Readiness release_sha** — H01-ready and H03-alias differ only in
  `release_sha`, the new oracle SHA.

Regenerated all artifacts that carry `source_sha`
(`deploy/rust/{market-policy,market-policy.manifest,mcp-catalog}.json`,
`rs/read/src/market/{curve_policy,orderbook_query}.json`, catalog vectors).
No case showed an unexplained difference.

## Step 4 — gates (all on the test host through the wrapper)

Every call ran alone and ended with a recorded exit marker.

| # | Gate | Result |
| --- | --- | --- |
| 1 | Python unit suite (`ENVIRONMENT=test … pytest tests/unit -q`) | **2350 passed** |
| 2 | `SQLX_OFFLINE=true cargo test --workspace` | **34 targets ok, 0 failed** |
| 3 | `cargo fmt --all -- --check` | **pass** |
| 4 | `cargo clippy --workspace --all-targets -- -D warnings` | **pass** |
| 5 | `scripts/rust_contract/sqlx_offline_check.py` (`cargo sqlx prepare --check`) | **PREPARE exit=0** |
| 6 | Combined gate `run_disposable --group all --rust-target all --require-complete-ledger --report docs/rust-phase1/ci-contract.json` | **all 11 groups passed, ledger OK, `failed_groups: []`** |
| 7 | `scripts/rust_contract/measure_lists` (release build, 240 requests) | **`require_latency_parity` passes** |
| 8 | `bash scripts/run_phase0_postgres_tests.sh` | **179 passed** (disposable cluster, 12m25s; the Phase 0 suites carried in by the merge) |

The combined gate initially failed four groups (orderbook_lists 21,
orderbook_snapshot, live_benchmarks 18, curves_canonical) on two real defects,
both fixed in commit 65ec9e41:

- `rs/read/sql/live_benchmarks.sql` did not project `carbon_intensity_method`,
  so the new B30/B100 ASK rows failed the biofuel branch and lost their
  live-slice benchmark — a null-versus-value difference against Python.
- `curve_policy.json` `seed_quotes` had the B30/B100 entries appended at the
  end; the curves oracle asserts the policy product order (B30, B100, then the
  rest). Reordered.

### p95, release build, 240 requests / 30 per cell, interleaved

`require_latency_parity` passes — **Rust p95 ≤ Python p95 in all eight cells**:

| Cell | Python p50 / p95 s | Rust p50 / p95 s |
| --- | --- | --- |
| list cold process | 1.140 / 1.370 | **0.725 / 1.023** |
| list warm | 1.178 / 1.581 | **0.750 / 0.968** |
| bids cold process | 1.892 / 2.510 | **1.395 / 1.765** |
| bids warm | 1.791 / 2.513 | **1.281 / 1.470** |

## Resume point

Task 13b leaves the Phase 0 convergence reproduced in Rust: the eight-product
catalog policy, the fixed-biofuel supplier-metadata clause on both the SQL and
pure paths, a re-recorded corpus at oracle `4ed68a2f`, and all gates green. The
superseded `f7b6c7b8` corpus remains until the PM approves its removal; the
combined-gate report is committed at `docs/rust-phase1/ci-contract.json`.
No push, live activation, or agent use.
