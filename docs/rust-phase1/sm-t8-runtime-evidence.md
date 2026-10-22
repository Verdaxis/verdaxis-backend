# Same-meaning Task 8 runtime checkpoint — 22 October 2026

Plan section: **Task 8: Add trading caching with a commit-safe hit check**, under
**Cache design and proof**. This is the runtime checkpoint only, not acceptance
of the route-wrapper step or an SM-R2 release.

## Source and regression

- `d408c1ea` adds `an_orderbook_entry_serves_only_until_its_own_expiry`.
  With the old runtime, it fails at the assertion that the family is stored
  with only the facts its query reads: four Rust cases pass, one fails;
  `PG_EXIT=1` (`work/t8-red-boundary-2.log`).
- `651329e4` uses `boundary_for(family, &clock)` at insertion. An unrelated
  unknown fact no longer prevents a proved family from being cached. A required
  unknown fact still bypasses storage. The regression checks a live hit and a
  reload after expiry without a writer.
- `02fe66d1` merges staging through `274bd3ab`, including PR #115. This lane did
  not edit the dependency constraints or lock file.

## Staging test-host gates

All commands ran through `verdaxis-staging-test` and `heavy run 3600`.
The source under test was `651329e41f55cb5acecd631f0f56dca953ca093c`.
No build or test ran on the Mac. These are correctness checks, not measurements.

| Gate | Exit | Local log |
| --- | ---: | --- |
| Focused boundary GREEN, same disposable driver as RED | 0 | `work/t8-green-boundary-1.log` |
| Formatting | 0 | `work/t8-gates-13.log` |
| Workspace/all-target clippy, warnings denied | 0 | `work/t8-gates-13.log` |
| Offline workspace tests | 0 | `work/t8-gates-12.log` |
| Seeded market-contract PostgreSQL suite, six cases | 0 | `work/t8-gates-13.log` |
| Entire `test_public_read_cache_events.py`, including two Rust readers and two database incarnations | 0 | `work/t8-gates-13.log` |
| Full Rust disposable PostgreSQL runner | 0 | `work/t8-rust-pg-13.log` |

Every listed call has a final `CALL_EXIT=0` marker. The inherited
`t8-gates-12.log` clippy command failed with exit `101` because it omitted
`RELEASE_SHA`; its offline step then passed. The corrected clippy command exports
`RELEASE_SHA=$(git rev-parse HEAD)` and `SQLX_OFFLINE=true` before running.
A successful wrapper marker alone is not the result of an inner gate.

## Review round — 22 October 2026

Each test below was committed before its implementation and run through the
same disposable driver (`test_rust_trading_cache_needs_every_authority_and_generation`)
on that commit and then on the implementation commit.

| Required behavior | Test commit | Before | Implementation | After |
| --- | --- | ---: | --- | ---: |
| With the runtime on, revoked, expired, mismatched, missing or rotated authority returns the database error (`unadmitted_connections_never_publish`) | `90ac6eb4` | 1 | `236e62f9` | 0 |
| Moving price summaries need rolling-window facts (`prices_require_a_known_rolling_departure`) | `6482deb2` | 1 | `beb905cc` | 0 |
| Overlapping snapshots both answer; the older fill does not replace the newer entry (`overlapping_snapshots_publish_without_replacing_the_newer_entry`) | `c0ca1bef` | 1 | `c2a27b0e` | 0 |
| Controlled-clock expiry, grace, rolling departure, Singapore day and late fill; barriers before insertion and publication with equal and reduced generations; distinct rows from two incarnations at one generation; two readers on each incarnation | `e2261248` | 1 | `16e1c1af` | 0 |

`an_older_fill_never_replaces_a_newer_entry` (`ac06b4a6`) is an offline unit
test of the cache's own ordering check from `c2a27b0e`. It fails with only that
check removed (exit 101) and passes on the final source. `4c9a7f97` and
`4af84626` change structure and formatting only.

Final gates on `4af84626a1ac687c1a57ce74e5e5a495fb997898` (M1 for formatting,
clippy and offline tests; staging for PostgreSQL suites):

| Gate | Host | Exit |
| --- | --- | ---: |
| Formatting | M1 | 0 |
| Workspace/all-target clippy, warnings denied | M1 | 0 |
| Offline workspace tests, 265 passed | M1 | 0 |
| Entire `test_public_read_cache_events.py`, 31 passed | staging | 0 |
| Seeded market-contract PostgreSQL suite, six cases | staging | 0 |
| Full Rust disposable PostgreSQL runner, 275 passed | staging | 0 |

## Resume point and limits

Trading caching remains inactive in serving routes. The prepared-budget listener
switch defaults off. The runtime hook, authority refresh, cache registry and
commit-safe read helper are available, but no route calls `run_trading_read`.
Do not edit the Task 9/10 route or SQL files until the PM accepts their uncached
handoff and it is merged here. Then derive the actual route boundaries in the
same snapshot, add wrappers and prove middleware behavior on hits. No activation,
live DDL, deployment or measurement is authorized by this checkpoint.

The CI-only draft is PR #114, base `staging`, head
`feat/rust-same-meaning-t8`. Its previous head `7c77a820` failed the dependency
audit and the `verdaxis_backup` phase0 case. Current-head CI must be read after
publication; a prior head's results do not qualify this checkpoint.
