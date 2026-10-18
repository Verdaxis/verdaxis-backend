# Task 14b source readiness

This work changes source only. It does not authorize activation, publication,
installation, a live comparison, or public routing. Decision 96: no measurements.

## Schema review

`p0_20261016_union` descends from `kycd_20261015_staging`. Its upgrade inserts
missing canonical product/delivery rows without changing existing values,
replaces `trades.ck_trades_snapshot`, widens the market-product checks on
`market_indications`, `fair_price_bands`, and `physical_stems`, and replaces
`verdaxis_validate_trade_snapshot()`. It removes or changes no queried table,
column, type, or read permission. The replacement trigger applies to writes;
Rust does not invoke it.

Reviewed query families in `rs/read/sql/` and inline SQL in `rs/read/src/`:

| Family | Compatibility with union |
| --- | --- |
| Catalog products/points | Exact identity joins to exported policy; added canonical rows already exist in that policy. Existing row values remain authoritative. |
| Shared order eligibility, keyed slices, private digest, lists/snapshot/map/count/distinct/listings, live benchmarks | No referenced column or type changes. Product/lane qualification stays policy-bound, not check-constraint-bound. |
| Formal trade snapshots, quarantine, tape, summary/latest/reference, curve trades | Snapshot column layout is unchanged. Expanded checks admit the distinct policy products; evidence predicates still use immutable snapshot identities. No trigger call. |
| Curve indications/bands/stems, depth/orders/benchmarks and MCP market reads | Signal check expansion changes admissible rows, not projection. Policy and trusted-ingestion predicates remain in force. |
| Ports/intelligence/news | Unaffected tables. |
| Runtime role/membership, connection budget, session settings, Alembic singleton and readiness | Unaffected system/schema metadata. The revision check consumes the compiled JSON allowlist. |

The allowlist is `deploy/rust/allowed-revisions.json`, compiled with `include_str!`
by `runtime/config.rs` and packaged as runtime/artifact policy. It retains the
previous checkpoint and adds the exact union checkpoint. It grants no production
activation: runtime configuration still refuses production.

Disposable contracts **do cover the union schema**: `run_disposable.py` calls
`verify_schema_drift.py`, whose disposable upgrade reaches Alembic head, including
this migration. Its historical runtime fixture label is not the migration target.
That is populated disposable correctness evidence, not proof of current live
staging data or permission to migrate staging.

## Gates

All runs use `verdaxis-staging-test` and `heavy run 3600`. No local tests.

- Schema test first run: exit 101 (missing required build `RELEASE_SHA`, not a red test).
- Corrected schema red: exit 101; exact union revision refused as expected.
- Schema green: exit 0; all 15 runtime tests pass, including production refusal.

## Staging policy

The committed staging policy now allows Rust at `127.0.0.1:8011`. No public
route, unit, installer, or deploy helper changed. Production reads neither
activation record, has no Rust manifest entry, and remains disabled. Tests
execute the existing policy loader for both staging and production.

- Policy initial red: exit 2 (missing test import; corrected before implementation).
- Policy corrected red: exit 1; disabled staging policy rejected as expected.
- First combined green: exit 1; 48 passed, one fixture readiness failure in
  `test_host_key_matching_the_pinned_digest_is_preferred`.
- Separate rerun: unit policy suite exit 0 (15 passed); release suite exit 0
  (34 passed). No source change was needed for the fixture readiness rerun.

## Remaining operator gates

Candidate artifact publication and independent candidate activation remain owned
by the PM. No deploy helper or artifact workflow change is included. The proposed
short comparison floor still needs explicit PM approval. Do not infer a live PASS
from these source checks.
