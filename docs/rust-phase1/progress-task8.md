# Rust Phase 1 progress — Task 8 (Lane: simple reads)

This file is the Task 8 record for the lane branch
`feat/rust-phase-1-lane-simple-reads`. It is deliberately separate from
`docs/rust-phase1/progress.md` so parallel lanes can merge without clashing.

## Task 8 — Ports, catalog, and news from the fixed pattern

- Date: 2026-10-15. Base commit: `9b7ef63`.
- Range covered: R01–R05 of the exact route inventory.
- Status: implemented and verified locally. No live host, deployment, push,
  agent, or background job was used. `.claude/` and `work/` stay untracked.

## Files

Created:

- `rs/read/sql/{ports,port,catalog_products,catalog_points,news}.sql`
- `rs/read/tests/simple_reads.rs` (offline vectors)
- `rs/contract/tests/simple_reads.rs` (the feature-gated group target)

Modified:

- `rs/read/src/routes/{ports,catalog,news}.rs`
- `rs/read/examples/contract_server.rs` (loads the committed policy)
- `tests/rust_contract/cases.json` (three fixture-independent news cases)
- `ARCHITECTURE.md`

## Implementation

- `ports.rs` serves `/api/ports` and `/api/ports/{port_id}` from
  `rs/read/sql/{ports,port}.sql`. The geography column is cast to geometry
  before `ST_X`/`ST_Y`, `location` is never selected, and the route adds no
  active filter and no ordering, matching the Python handler. The intelligence
  join key is the presence marker, so a port without an intelligence row
  serializes an explicit `intelligence: null`.
- `catalog.rs` serves `/api/catalog/products` and `/api/catalog/delivery-points`
  by joining `products` and `delivery_points` to the policy artifact with
  `jsonb_to_recordset`, then projecting through the shared
  `market::policy::project_catalog`. No product code, name, or UUID is written
  in Rust. `min_lot_size` is projected as text at its stored scale.
- `news.rs` serves `/api/news` from `rs/read/sql/news.sql` and writes the
  ten-key dict contract in Python's dict order. `limit` and `min_relevance` go
  through the shared Python-compatible validator in `runtime/http.rs`;
  `category` is read from the raw query text because the validator has no text
  kind, and the empty string keeps Python's falsy "no filter" behaviour.
- All three modules run their database work through
  `runtime::admission::run_admitted` and map failures with the market-path
  classifier (`database_error`).
- Two UTC formatters are needed. Pydantic v2 writes `...Z`; Python's
  `datetime.isoformat()` writes `...+00:00`. Both omit the fractional part at
  zero and always write six digits otherwise. Verified against the installed
  Pydantic and re-verified end to end by the probe below.

## Verifier adaptation (deviation from the plan's file list)

- The plan names one group per route family and a `--rust-target` flag. The
  implemented `run_disposable.py` dispatches **groups**, so all five routes are
  compared as the single `simple_reads` group, and the group target is
  `rs/contract/tests/simple_reads.rs`, not `rs/read/tests/simple_reads.rs`.
- Task 8 must not edit manifests, so the group target carries
  `#[cfg(feature = "contract-http")]` instead of a `[[test]]` entry. Without
  the feature the file compiles to nothing, so `cargo test --workspace` stays
  offline-safe; with it the target runs through the comparator exactly like
  `health` and `echo`.
- `rs/read/tests/simple_reads.rs` holds the offline vectors (plan path, no
  manifest change needed).

## Findings for the packager (PM)

1. **The contract server loaded an empty policy.** `examples/contract_server.rs`
   built `MarketPolicy` with no products or points, so R03/R04 could only ever
   answer `[]` against a populated Python catalog. Task 8 loads the committed
   `deploy/rust/market-policy.json` through `load_policy` with the artifact's own
   recorded source SHA, the same attestation Task 5's tests use. This is a
   shared Task 3/4 file; the change is ten lines and additive. The serving
   binary (`rs/read/src/main.rs`) still has the same empty-policy placeholder —
   Task 13's assembly must fill it. **Cost if wrong:** if the packager prefers a
   different loader seam, this hunk moves; the routes read `AppState.policy`
   either way.
2. **The plan's checkpoint command cannot run as written.**
   `run_disposable.py` has no `--rust-target` option, and the default
   `--seed-mode corpus` fails closed because `seed_corpus.py` does not yet
   construct the `ports_intelligence_news` family. Task 8 ran the equivalent
   with `--seed-mode ordinary` (15 cases, all matching).
3. **The recorded `simple_reads` golden was captured under the ordinary seed**,
   so R01, R02, and R05 prove the empty-table, unknown-key, repeated-key, 404,
   422, and encoding paths only. Implementing the `ports_intelligence_news`
   family would invalidate that golden and force a corpus re-record; a re-record
   under a new source SHA would change Task 5's policy `source_sha` (read from
   the single golden directory), which is outside Task 8's authority. This needs
   a PM decision. **Cost if wrong:** the family and a re-record are a small
   follow-up, but the policy/vector chain and every other group's golden must be
   regenerated together.
4. **`ports.location` is NOT NULL** (revision `411b38418aea`), so the plan's
   "null location" case is unrepresentable in the deployed schema. `lat`/`lng`
   keep their defensive null handling; only the intelligence row is optional.
5. **News admission.** Python's middleware lets `/api/news` bypass the public
   read gate, but decision 11 forbids route code from touching the plain pool,
   so `run_admitted` takes a slot for news. Under full contention Rust could
   answer news with 503 where Python would serve it. This is not covered by
   AD-01 or AD-02 and is recorded, not changed.
6. **Trailing slashes.** `MOUNTED_GET_PATHS` in `runtime/http.rs` still lists
   only the three health paths, so `/api/ports/`, `/api/news/`, and the two
   catalog paths are 404 instead of Starlette's 307. No corpus case covers it;
   the list belongs to Task 13's exact-routing assembly.
7. **`cases.json` gained three fixture-independent cases** for the news query
   parser: `R05f-news-min-relevance` (422), `R05g-news-relevance-four` (200),
   `R05h-news-empty-category` (200). The simple_reads group now runs 15 cases.
8. **One observed flake.** The first `run_postgres.sh` run failed the Task 3
   test `cancelled_query_keeps_slot_until_drop` (one fresh request answered 500
   instead of 200) while the machine was under heavy load from parallel lanes;
   the immediate re-run passed 65/65. The target does not touch Task 8 code.
   Evidence: `work/rust-phase1/task8-postgres.txt` (failure) and
   `work/rust-phase1/task8-postgres2.txt` (green re-run).

## Evidence and checks

All Cargo commands used local Rust 1.95, the harness venv
`work/rust-phase1/venv312` (CPython 3.12.2), and a fresh
`export RELEASE_SHA=$(git rev-parse HEAD)`, in the foreground.

- RED: `cargo test -p verdaxis-read --test simple_reads` failed with the
  missing module exports (exit 101).
- RED: `run_disposable --group simple_reads` (the plan's command) failed at
  `seed_corpus` with the unimplemented `ports_intelligence_news` family. That
  failure is still reproducible, because the family is still unimplemented.
- RED: `run_disposable --group simple_reads --seed-mode ordinary` reported 34
  differences (404 against every Python response).
- GREEN: `run_disposable --group simple_reads --seed-mode ordinary`:
  **pass**, `contract group simple_reads OK: 15 case(s)`.
- GREEN: `cargo test -p verdaxis-read --test simple_reads`: **pass**, 8 tests.
- GREEN: `cargo test --workspace`: **pass**, 60 tests (http_contract 15,
  identity 1, market_contract 16, policy 12, request_log 1, runtime 7,
  simple_reads 8), 0 failures.
- `cargo fmt --all -- --check`: **pass**.
- `cargo clippy --workspace --all-targets -- -D warnings`: **pass**.
- `RUST_TEST_PYTHON=… bash rs/read/tests/run_postgres.sh`: **pass**, exit 0,
  65 tests across 17 targets. Evidence: `work/rust-phase1/task8-postgres2.txt`.
- Python: `pytest tests/rust_contract -q`: **pass**, 42 tests.
- Python: the Task 8 ledger row sources
  (`test_public_market_catalog_contract.py`, `test_catalog_models.py`,
  `test_news_feed.py`): **pass**, 35 tests.
- Python: `pytest tests/unit -q`: **1921 passed, 19 failed**. All 19 are the
  pre-existing `tests/unit/test_deploy_health.py` failures recorded in Task 4
  (macOS `/bin/bash` 3.2 lacks `mapfile`); the count and the failing file are
  unchanged from Task 6.
- **Data-bearing probe** (`work/rust-phase1/simple_reads_probe.py`, temporary,
  uncommitted): 10 cases against a disposable seeded database with fixture
  ports and news — **0 differences**, including the body byte digest. It covers
  an inactive port still being returned, `intelligence: null` versus a present
  intelligence object, both date formats, a null summary, the category filter,
  the empty category, the relevance filter, the row limit, and an unknown query
  key. Evidence: `work/rust-phase1/probe-final.txt`.
- `git diff --check`: pass.

## Resume point

Routes R01–R05 are complete on this lane. Task 13 registers the three modules
in the serving binary without changing their SQL, and must also load the market
policy there. Tasks 9, 10a, and 10b follow the same pattern: group target in
`rs/contract/tests/` with the `contract-http` gate, offline vectors in
`rs/read/tests/`, SQL in `rs/read/sql/`, and every database call through
`run_admitted`.
