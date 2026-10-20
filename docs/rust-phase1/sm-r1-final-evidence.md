# Task 11 evidence — same-meaning release 1 (SM-R1)

Scope: same-meaning Tasks 1–7 on `feat/rust-same-meaning`, trading caching
off. Task 8 (trading caching), Task 9 (curve, reference-export and MCP slice
costs) and Task 10 (orderbook aggregation and kept-page costs) are SM-R2 and
are not in this record.

Task and section names follow `outputs/Verdaxis Rust Backend Rewrite - Phase 1
Same-Meaning Plan - 2026-10-18.md`.

- Evidence tree: `940c41dc50c12b2e800ab9bdb2efc562f2ad6b68`, plus the
  test-harness commit `d5396e0f` and this document.
- Trading caching stayed disabled. No deploy, live DDL or production change
  ran.
- Measurement: **not run (decision 110)**. The founder deferred load testing
  to after cutover, so Task 11 has no latency, memory or capacity cell. No
  measurement call started or waited.

## 1. Phase 2 authenticated transaction

Both options in the PM brief were inspected. The branch now carries the fix
from `origin/feat/rust-phase-2`, applied as a cherry-pick of `bae1e813`
(`91fa856e`). The whole branch was not merged: it is 26 commits ahead and
carries unrelated Phase 2 route work that SM-R1 must not ship. The
cherry-pick is exact for the changed file:
`git diff bae1e813^ 8af79e75 -- rs/read/src/auth/transaction.rs` is empty, so
the branch's copy was identical to the parent.

The fix replaces the raw `BEGIN` with sqlx's transaction guard
(`begin_with("BEGIN ISOLATION LEVEL READ COMMITTED READ WRITE")`). The guard
queues a `ROLLBACK` when it drops during an unwind, so a panicking request
cannot return an idle-in-transaction connection to the pool. The explicit
commit, the rollback-then-ping close rule and the ambiguous-commit close are
unchanged.

Evidence, `auth-state-matrix` disposable scenario on the staging test host,
exit `0`:

| Target | Result |
|---|---|
| `auth_transactions` | 2 passed |
| `auth_transaction_panic` | 2 passed (panic in `work`, panic in the account-row decode) |
| `auth_tokens` | 2 passed |

The panic tests failed on the first attempt because the disposable Phase 2
scenario migrated the database as the application role. The cache
invalidation trigger is `SECURITY DEFINER` and was therefore owned by a role
without write authority, so the fixture's user update was refused. Production
migrates as the migrator role. Commit `940c41dc` makes the scenario create the
roles and move schema ownership before the migration, matching production;
the tests then pass. This changes only the disposable harness, not runtime
code.

## 2. Meaning comparator over every Phase 1 route and MCP tool

Full gate, source `940c41dc`, staging test host, exit `0`:

```
venv/bin/python -m scripts.rust_contract.run_disposable \
  --group all --rust-target all --mixed-caddy --require-complete-ledger \
  --profile phase1-meaning-v1 \
  --rules-sha256 f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e \
  --report work/smr1-contract.json
```

- Profile `phase1-meaning-v1`, rules digest
  `f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e` — the
  profile the CI contract job runs.
- All eleven groups passed: `curves_canonical`, `curves_legacy`, `echo`,
  `health`, `live_benchmarks`, `mcp`, `orderbook_lists`, `orderbook_map`,
  `orderbook_snapshot`, `prices`, `simple_reads`.
- Assertion ledger OK: 336 discovered assertions each map to a target or a
  reviewed exclusion; no ledger error.
- The MCP group exercised the three tools; the route inventory is proven by
  `tests/rust_contract/test_phase1_meaning_inventory.py`, which requires
  exactly 27 routes and 3 tools, compares every request with `cases.json` and
  requires a comparator id, codec, cache class and Caddy group for each.
  The four Phase 1 meaning test modules passed **41 tests, exit `0`** on the
  final harness commit.

## 3. Cache authority, fencing and failover

Caching stays disabled; these tests prove the authority, fencing and failover
machinery and the migration's invalidation contract, not an activated cache.

Rust workspace PostgreSQL suite (`rs/read/tests/run_postgres.sh`), staging
test host, exit `0`, **253 passed, 0 failed**:

| Target | Passed | Covers |
|---|---|---|
| `cache_authority` | 13 | authority record identity, fence changes, listener replacement, expiry, generation rollback, unbudgeted listener refusal |
| `cache_events` | 3 | explicit listener budget, batched invalidation, one-slot requirement |
| `cache_events_postgres` | 4 | triggers under every session role, authority mismatch refusal, lost connection and catch-up, commit-time notification |
| `cache_failover_postgres` | 6 | stale epoch, fenced old primary, promotion without cache reuse, restore rotation, uncertain authority, missing infrastructure |

The Python PostgreSQL suite (`scripts/run_product_analytics_postgres_tests.sh
tests/postgres`), same host, exit `0`, **298 passed, 0 failed**. It includes
`test_public_read_cache_events.py` and `test_public_read_cache_failover.py`
(the two-incarnation failover cases that start the Rust suites) and the
migration tests.

## 4. Migration checkpoint and disposable upgrade

`prc_20261018_cache`:

- `revision = "prc_20261018_cache"`, `down_revision = "p0_20261016_union"` —
  directly after the revision production runs.
- It is the single Alembic head.
- `deploy/rust/allowed-revisions.json` is exactly
  `["kycg_20261014_staging", "p0_20261016_union", "prc_20261018_cache"]`.

New test `tests/postgres/test_market_migrations.py::test_cache_checkpoint_follows_production_revision`
asserts the head, the direct parent, the allowlist and a disposable upgrade
from production's current revision: it upgrades an empty database to
`p0_20261016_union`, checks `alembic_version`, upgrades to `head`, checks
`prc_20261018_cache`, and checks the three cache tables exist. It passed
alone (exit `0`) and inside the 298-test suite.

Production stays at `p0_20261016_union` for SM-R1: the release configuration
validator still requires that revision, and no live migration runs in this
lane.

## 5. Gates and exit codes

| Gate | Result |
|---|---|
| Phase 2 `auth-state-matrix` scenario | exit `0` (6 tests) |
| Meaning comparator, all groups, `phase1-meaning-v1` | exit `0` |
| Phase 1 meaning test modules | exit `0`, 41 passed |
| Rust PostgreSQL workspace suite | exit `0`, 253 passed |
| Python PostgreSQL suite | exit `0`, 298 passed |
| Migration checkpoint test alone | exit `0`, 1 passed |
| Measurement | not run (decision 110) |

CI on PR 90 runs the offline Rust tests, formatting and clippy, the PostgreSQL
suites and the same contract gate; its result on the pushed head is the
final-tree confirmation.

## 6. Independent read-only reviewer checklist

1. Confirm the PR head and that the branch contains exactly the Phase 2 fix
   plus test-harness and evidence commits: `git log --oneline e2fe7dfe..HEAD`.
   Confirm `git diff bae1e813^ 8af79e75 -- rs/read/src/auth/transaction.rs`
   is empty and that no Phase 2 route module is added by the cherry-pick.
2. Confirm `rs/read/src/auth/transaction.rs` contains no
   `execute("BEGIN")`: the transaction starts through the sqlx guard and the
   commit, rollback-then-ping and ambiguous-commit paths are intact.
3. Read the PR checks. Require `contract` on the head to report the
   `phase1-meaning-v1` profile with rules digest
   `f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e`, all
   eleven groups passing, and the ledger check passing.
4. Re-run or read `tests/rust_contract/test_phase1_meaning_inventory.py` and
   confirm 27 routes, 3 MCP tools and exact corpus equality (41 tests).
5. Re-run the four cache targets and confirm the six failover cases by name;
   confirm the Python suite includes the cache event and failover files.
6. Re-run `test_cache_checkpoint_follows_production_revision`, then read
   `alembic/versions/prc_20261018_cache.py` and
   `deploy/rust/allowed-revisions.json`; confirm one head and the direct
   parent `p0_20261016_union`.
7. Confirm production configuration still requires `p0_20261016_union`
   (`scripts/prepare_rust_runtime.py`, `rs/read/src/runtime/config.rs`) and
   that no deployment unit passes `--cache-listener`.
8. Confirm the diff contains no deploy, DDL or production-config change, and
   that no measurement exists: decision 110 deferred it to after cutover.
9. Confirm the harness commits (`run_disposable.py` role bootstrap and
   `run_postgres.sh` skips) touch tests only. The skipped market-policy matrix
   runs in `rs/read/tests/run_market_contract.sh` — 6 passed, exit `0`.
