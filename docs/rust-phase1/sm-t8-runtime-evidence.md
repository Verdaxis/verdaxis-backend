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

Correction (review round 2): only the late-fill case of that row failed because
of the runtime. The other clock cases and the rotation barriers passed on the
earlier runtime too, so they are coverage. The two rotation variants ran the same
revocation path: the successor's activation reached the process only when a
refresh happened to be due, and no successor read set the high-water mark, so
"equal and reduced generations" did not change what the paused fill met. The Singapore
case set its boundary by hand. Round 2 below replaces both tests.

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

## Review round 2 — 22 October 2026

Batched as decision 126 requires: the tests-only commit `95e99638` ran RED, and
the final commit `37fdd763` ran GREEN and every gate. The fix commits were not
run alone.

| Required behavior | Test | RED on `95e99638` | Implementation | GREEN on `37fdd763` |
| --- | --- | --- | --- | --- |
| The tape's seven-day lookback ends an entry when a trade ages out, with no write | `a_trade_ages_out_of_the_tape_window_without_a_write` | failed (served the aged trade) | `8239e887` | ok |
| A moving-window family stores nothing for "no window" or an unknown departure | `moving_windows_without_a_derived_departure_bypass_storage`; unit `moving_windows_need_a_derived_departure`, `unknown_dependency_bypasses_instead_of_guessing` | failed (prices: no window; tape: both) | `8239e887` | ok |
| Each family requires the predicates its query binds (curves read resting orders) | unit `every_family_requires_the_time_predicates_its_query_binds` | failed | `8239e887` | ok |
| An expiry between the route's clock and the end of the read is answered, not stored; a change reported at or before the clock fails closed | `an_expiry_between_the_route_clock_and_the_read_is_not_stored`; unit `a_reported_change_at_or_before_the_clock_fails_closed` | failed (stored) | `a89c1845` | ok |

RED exits: offline unit tests 101 (four failed, six passed) on the M1; the
disposable PostgreSQL driver 1 (three failed, eleven passed) on staging.

Limits of this evidence:

- The tests-only commit calls the earlier interface. `a89c1845` adds the clock
  parameter, so it changes those call sites; the assertions stay the same. It
  also adds the case of a loader that ignores its clock, which has no separate
  RED run.
- Coverage only (passed before and after): the clock cases, which now derive
  each fact from the rows their own predicate kept (the Singapore case through
  `certificate_boundary`), and the rotation barriers. In each rotation variant a
  successor read sets the high-water mark equal to the paused fill's generation
  or one below it before the release. All four variants stop at the same fence
  check; they differ in state, not in code path. With equal generations, a check
  on generation numbers alone would accept the fill.
- The late-fill case uses the observer seam. In serving, `Instant::now()` and
  the observer agree; the case proves that insertion checks the current time.
- `calendar_boundary` is proved only by unit tests.

| Gate on `37fdd76320fd07eec65ea7518fcf9a28a93c99de` | Host | Exit |
| --- | --- | ---: |
| Formatting | M1 | 0 |
| Workspace/all-target clippy, warnings denied | M1 | 0 |
| Offline workspace tests, 270 passed | M1 | 0 |
| Entire `test_public_read_cache_events.py`, 31 passed | staging | 0 |
| Seeded market-contract PostgreSQL suite, six cases | staging | 0 |
| Full Rust disposable PostgreSQL runner, 280 passed | staging | 0 |
| `cd rs && cargo audit` | staging | 0 |

## Resume point and limits

Trading caching remains inactive in serving routes. The prepared-budget listener
switch defaults off. The runtime hook, authority refresh, cache registry and
commit-safe read helper are available, but no route calls `run_trading_read`.
Do not edit the Task 9/10 route or SQL files until the PM accepts their uncached
handoff and it is merged here. Then derive the actual route boundaries in the
same snapshot, add wrappers and prove middleware behavior on hits. No activation,
live DDL, deployment or measurement is authorized by this checkpoint.

Open items for the route-wrapper step:

- Capture one `TradingClock` before admission (the instant the route also uses
  for validation), pass it to `run_trading_read` and bind only that clock.
- Payloads carry request-time fields (`generated_at` in prices, curves and the
  orderbook snapshot). Rebuild them per request or keep them out of the cached
  payload.
- Curves also use UTC dates (the board's months ahead and the spot window start
  use the UTC date). `calendar_boundary` covers only the Singapore midnight, so a
  curves wrapper must report the earlier of the two midnights.
- `rolling_window_departure` takes whole days; the price summary's lookback is in
  hours (1 to 168).
- With the switch on, an unadmitted process fails trading reads. Report it not
  ready, or keep it out of routing, until its listener attaches (see
  `cache-dependencies.md`, "Availability while unadmitted").

The CI-only draft is PR #114, base `staging`, head
`feat/rust-same-meaning-t8`. Its previous head `7c77a820` failed the dependency
audit and the `verdaxis_backup` phase0 case. Current-head CI must be read after
publication; a prior head's results do not qualify this checkpoint.
