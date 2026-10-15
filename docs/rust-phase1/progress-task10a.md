# Task 10a — Legacy curve JSON and CSV

Date: 2026-10-16. Base: `a127672`. Branch: `feat/rust-phase-1-lane-10a`.

## Status and resume point

R07/R08 are implemented and pass the local ordinary-seed gate. **Final acceptance
is pending Task 4b's populated corpus**, as required by decision 15. No canonical
curve, board, benchmark, price, writer or deployment work was done.

After integration, rerun `curves_legacy` on the populated corpus in both KYC
states. Do not relabel the old golden bytes as this source revision. Task 13
still owns global method/slash routing, including the known HEAD behavior.

Task 4b's accepted golden is on the build branch, not merged into this lane.
The PM owns that merge and the populated acceptance rerun.

After Task 9's required `DB_SESSION_TIME_ZONE` fix merges, run disposable
Python/Rust differential tests with UTC and Asia/Singapore sessions across
Singapore midnight (`2026-10-15T15:59:59.999999Z` and `2026-10-15T16:00:00Z`).
Verify the configured pool's `SHOW TimeZone`. Include a FAME certificate valid
through 15 October and exact expiry/grace boundaries, with both KYC switch
states. Compare legacy JSON and CSV at each instant. Outputs must match Python
and must not change merely because the session zone changes. Legacy routes have
no day buckets; their certificate date is explicitly Singapore-local and their
window codes do not depend on the database zone.

## Implementation and architecture note

- `routes/curves_legacy.rs` mounts the two legacy paths. It uses one captured
  instant, the common query binder and `run_admitted`. CSV construction completes
  inside the admitted task. Decimal prices remain strings with stored scale;
  midpoint/spread use half-even rounding and two decimal places. Aliases merge
  before final projection and canonical sorting. Null CSV cells are empty and
  rows end in LF. There is no benchmark fallback or typed benchmark qualification.
- New `market/legacy_curve.rs` and `sql/legacy_curve_orders.sql` define a named
  REAL-only relation without active catalog joins. It returns an `OrderQuery`,
  uses the existing binder, and requires the requested product UUID.
- New `sql/current_order_rule.sql` extracts expiry, owner/KYC and shallow typed
  declaration rules from `sql/public_orders.sql`. Both relations substitute that
  same fragment. Declaration profiles use policy product IDs, matching Python's
  current-order helper. The existing public relation still applies strict
  catalog identity, lane and economic provenance before its consumers run.
- Minimal shared edits: `market/mod.rs` registers the new module;
  `market/visibility.rs` composes the shared fragment and lets sibling market
  modules construct `OrderQuery`. No public query constructor or binder signature
  changed. Tail binds still begin at `$14`.
- `sql/curves/legacy_orders.sql` groups the named relation by raw window and side.
  No route contains an admission predicate or executable product identity.

This task-local architecture note replaces an edit to `ARCHITECTURE.md` because
this lane's explicit file ownership excludes that integration-owned document.

## Tests and evidence

Every Cargo invocation used a fresh full `RELEASE_SHA` from this worktree.
Temporary logs are in ignored `work/rust-phase1/task10a-*.log`.

1. Added `rs/contract/tests/curves_legacy.rs` first. The ordinary-seed runner
   failed: 0 passed, 1 failed, absent handler comparison. `task10a-red.log`.
2. Added arithmetic/CSV tests before implementation. They failed on unresolved
   `curve_csv` and `midpoint` imports. `task10a-unit-red.log`.
3. Implemented the relations and routes. The original four contract cases pass.
4. Added an explicit shared-relation comparison using the existing Task 6
   fixture matrix, through `rs/read/tests/legacy_curve_oracle.py`. It compares
   exact JSON and CSV for all seven product IDs: **14 comparisons pass**. It
   separately proves a current REAL BID on an inactive product remains in the
   result. The matrix includes inactive points, noncanonical identities,
   off-spec scalar inputs, missing scalar declarations, shallow-visible typed
   declarations, rejected/expired orders and non-REAL provenance. The initial
   probe failed its fixture database attestation; the corrected probe supplies
   the already-attested runner database name rather than removing the guard.
5. The existing shared market runner passes **2 tests**, including both KYC
   states. This checks that extraction did not change public-order selection.
6. Final `cargo fmt --all -- --check`: pass.
7. Final `cargo clippy --workspace --all-targets -- -D warnings`: pass.
8. Final `cargo test --workspace`: **77 passed**. Includes 3 local curve tests:
   midpoint ties/one-sided markets; exact CSV header, empty/null, quoting and LF;
   aliases, full two-sided projection, volumes/counts and window ordering.
9. Final `RUST_TEST_PYTHON=$PWD/venv/bin/python bash rs/read/tests/run_postgres.sh`:
   **84 passed**. Its two shared matrix tests are explicitly filtered there and
   ran in their dedicated migrated runner in step 5.
10. Final `venv/bin/python scripts/rust_contract/run_disposable.py --group
    curves_legacy --seed-mode ordinary`: **1 Rust contract test passed**, covering
    the four corpus cases plus the 14 matrix comparisons. No differences.
11. `venv/bin/python -m pytest tests/rust_contract -q`: **59 passed**, 5 warnings.
12. Python `tests/unit/test_curves.py` with explicit test/SQLite settings:
    **35 passed**, 1 warning. Assertion ledger: OK. `git diff --check`: pass.

One intermediate workspace run failed because the extracted SQL comment had a
semicolon and the existing single-statement test rejects all semicolons. Removed
that comment punctuation; the test passes without changing its assertion.

## Deviations and boundaries

- The runner has no `--rust-target` option. Used its existing group dispatch and
  a feature-gated contract target, following Task 8, without editing manifests
  or server dispatch. The offline tests remain in the planned read test file.
- Shared SQL was edited only to extract the common rule; a copied predicate in
  the new relation would violate decision 16. The independent matrix comparison
  and existing market comparison both pass.
- No golden publication: Task 4b owns the populated recording. No assertion
  ledger change was needed; its existing curve mapping remains valid.
- No full Python unit suite or other route groups were run: they are outside
  Task 10a. All requested Task 10a checks ran. Populated acceptance was not run,
  per the explicit ordinary-seed instruction.
- All databases were disposable pinned PostGIS containers on loopback. Runners
  remove their containers at exit. No staging/prod contact, push, package install,
  skill invocation, subagent or other lane edit was performed.
