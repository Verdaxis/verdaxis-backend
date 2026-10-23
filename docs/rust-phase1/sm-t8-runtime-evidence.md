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

## Review round 3 — 22 October 2026

The tests-only commit `51c0888cc5cc068690e80664d266be42f362f50b`
adds a public-helper schedule for order expiry and tape departure. Request A
captures the earlier clock and waits after snapshot admission. B advances past
the row's departure and stores an empty result, then releases A. No writer
changes the generation. RED returned zero for A in both cases with one load,
instead of one with two loads (PostgreSQL driver exit 1; fourteen cases passed,
one failed). GREEN on `d35db8cebe179575f11ad5028272cb12346c1a25`
returns A's earlier row and preserves B's empty entry for the next request.

`dead0672` stores each entry's evaluation clock, checks the request against its
lower bound and upper boundary, and orders equal-generation replacement by
evaluation time. Authority, current-time expiry, lease and fence checks remain.
The extra unit case for same-generation replacement was added with the fix and
has GREEN coverage, not separate RED evidence. `d35db8ce` applies remote rustfmt.

| Gate on `d35db8cebe179575f11ad5028272cb12346c1a25` | Host | Exit |
| --- | --- | ---: |
| Focused PostgreSQL GREEN | staging | 0 |
| Formatting | remote M1 | 0 |
| Workspace/all-target clippy, warnings denied | remote M1 | 0 |
| Offline workspace tests | remote M1 | 0 |
| Full Python cache-event suite, 31 passed | staging | 0 |
| Seeded market contract, six passed | staging | 0 |
| Full Rust PostgreSQL runner | staging | 0 |
| `cd rs && cargo audit` | remote M1 | 0 |
| Full Python unit suite, 2534 passed | staging | 0 |

The round-2 Python release interruption failure is inherited and timing-sensitive;
its test-hygiene fix belongs to a separate lane. It did not fail in this run.
Python emitted existing warnings, including an aiosqlite thread warning.

Host exception: the M1 returned between calls, so `verdaxis-m1-test` routed the
first batch to the remote M1, not staging. Its Linux-only gates could not run,
and the full Python suite had the documented macOS Bash failures. Those results
are not Linux gate evidence. The Linux gates were then run on staging. A network
interruption hid the original staging run, which later terminated with exit 143;
a queued retry completed every Linux gate with batch and wrapper exit 0. The
successful staging batch records RED at 22:18:15Z and final completion at
23:03:43Z on 22 October. Raw logs and exact commands remain in ignored `work/`.
No build or test ran on the operator Mac; no measurements were run.

## Review round 4 — 23 October 2026

`effda3e4817208710f1ccf54a6de3e4165f81eaa` commits the tests
before the fix. The controlled clock can advance wall and monotonic time
separately. The public-helper case covers order expiry and tape departure:
load once, hit with a distinct forward clock, advance wall time past departure
while monotonic time remains before the stored boundary, then read twice.
No generation changes. RED returns `[1, 1, 1, 1]` with one load for each
family, instead of `[1, 1, 0, 0]` with two loads. Fifteen Rust cases pass;
only the new case fails. The unit case also rejects the exact wall deadline,
not just an instant after it.

`e5e76e44` reconstructs the wall deadline from the stored evaluation pair and
monotonic boundary, using checked arithmetic. The requesting wall clock must
be strictly before it. Unknown or unrepresentable boundaries miss; there is
no equality requirement, tolerance or TTL. Existing monotonic, lower-bound,
authority, lease, generation and fence checks remain. `efca442d` applies the
remote formatter patch.

| Check | Source | Host | Exit |
| --- | --- | --- | ---: |
| Unit RED | `effda3e4` | remote M1 | 101 |
| Public-helper PostgreSQL RED | `effda3e4` | staging | 1 |
| Unit GREEN | `e5e76e44` | remote M1 | 0 |
| Formatting | `efca442d` | remote M1 | 0 |
| Warnings-denied clippy | `efca442d` | remote M1 | 0 |
| Offline workspace (includes unit GREEN) | `efca442d` | remote M1 | 0 |
| Full Python cache-event suite, 31 passed; Rust cache cases, 16 passed | `efca442d` | staging | 0 |
| `cd rs && cargo audit` | `efca442d` | staging | 0 |
| Seeded market contract, six passed | `efca442d` | staging | 0 |
| Full Rust PostgreSQL runner | `efca442d` | staging | 0 |

The first M1 batch found formatting differences and could not fetch the RustSec
advisory database. Formatting was corrected and final-source Rust gates rerun;
the advisory fetch still failed on M1. The exact audit command passed on staging.
One staging call waited for its slot, then ran RED and every Linux gate without
a duplicate call. Its RED exit is at 01:07:33Z, GREEN at 01:14:50Z and final
batch/wrapper exit 0 at 01:37:59Z on 23 October. Exact source, commands, UTC times
and exits are in ignored `work/r4-*.log` and the matching gate scripts.
The full Python unit suite was not rerun: no Python or shared input changed.
No build/test ran on the operator Mac; no measurements or activation occurred.

## Release 2 route integration — 23 October 2026

Accepted runtime checkpoint: `0f01d4c6`. Merged accepted Task 9–10 `c599dddb`
with `--no-ff` at `509c13b0`, then staging `3bd89ae9` at `c40de2dc`.
Both merges were conflict-free. Reviewed the combined diagnostic accessors,
exact named test skips, shared harness normalization and legacy JIT transaction.
There was no Cargo lock resolution or manual conflict edit.

`runtime/trading_routes.rs` adapts only the assigned snapshot/list/map, prices
and canonical-curve HTTP handlers. A per-AppState bounded cache uses route,
complete raw query (or parsed curve query), configuration/policy identity and
captured UTC date. The date separates normalized calendar aliases. Successful
JSON alone is retained; generated timestamps and CSV/HTTP rendering occur after
lookup. Private/write, listings, tape and MCP paths remain outside this adapter.
The prepared-budget switch remains off by default; the disabled branch calls
the original admitted loader without boundary or cache metadata SQL.

Enabled reads derive conservative global temporal minima in the runtime's
repeatable read-only snapshot. They cover every future order/grace expiry,
Singapore certificate midnight, rolling trades (including quarantine), curve
7/30/90-day signal departures, seven-day freshness labels and UTC/Singapore
calendar rules. Global unrelated rows can shorten reuse, not extend it.
Inclusive rolling equality and unproved facts bypass storage. Existing loaders
use nested savepoints, retaining transaction-local JIT and projection behavior.
The snapshot's transaction characteristic statement runs only on the disabled
path; the enabled outer transaction already establishes those characteristics.

### Tests and limits of RED

The first tests-only checkpoint `3825e4f3` failed on request timestamps, not the
missing wrapper, and is not the behavioral RED claim. Corrected tests-only
checkpoint `d42ab531` contains the repaired fixture but no wrappers. It failed
with `route has no proven wrapper` on the actual snapshot HTTP handler (driver
exit 1, 06:21:53Z). This is one route-level RED; the other route cases were
blocked by that assertion, not individually run RED.

GREEN exercises sixteen actual HTTP route variants: enabled/disabled payload
parity, cold storage and warm requests, committed price changes before listener
event delivery, exact order-expiry equality, revoked-authority failure, and
successor reads with equal/lower generations. Existing runtime tests retain
the two-process/two-incarnation and stale-publication proof. The route test
compares generated-time-free business payloads; separate runtime clock tests
retain wall/monotonic interval coverage. It does not claim a speed measurement.

Two fixture-only corrections were needed: restore the disposable generation
above retained event keys before further writes, and supply the required `side`
query on product counts. These do not change production wrappers. On final
fixture source `357d333c`, the full cache-event suite passes (32 Python tests).

### Gate ledger

Production wrapper source is `3cc01606`; subsequent code changes are test-only
fixture corrections and a merge retaining the tests-only RED checkpoint.

| Gate | Source | Exit / result |
| --- | --- | --- |
| fmt, warnings-denied clippy, offline workspace | `3cc01606`, remote M1 | 0 each |
| M1 audit | `3cc01606` | 1, RustSec network fetch failed |
| market contract | `3cc01606`, staging | 0 |
| Rust PostgreSQL runner | `3cc01606`, staging | 0 |
| Python unit + monitor + rust_contract | `3cc01606`, staging | 1; only the two inherited Phase 2 failures |
| exact `cd rs && cargo audit` | `3cc01606`, staging | 0 |
| Phase 1 meaning gate, once | `3cc01606`, staging | 1; only prices / `T9-date-year-zero` |
| large prices simulation | `3cc01606`, staging | 0; 56 pairs, zero differences, no inconclusive group, pass |
| fmt and clippy after fixture correction | `b07e5f90`, staging | 0 each |
| full cache-event suite | `357d333c`, staging | 0; 32 passed |

Inherited Python failures: `test_verifier_has_no_issuing_or_database_interface`
and `test_source_digests_and_exact_exclusions`. The meaning failure reports
body/header/status differences for `T9-date-year-zero`; every other group,
including canonical and legacy curves, passed. The date parser is unchanged
from the accepted staging tree; this result is recorded, not waived or fixed
by modifying Python/comparator rules. The full meaning gate was not repeated.
Large simulation uses the staging harness fix and is now conclusive.

Raw exact commands, UTC times, SHAs and exits are in ignored
`work/integration-*-gates.log`, `work/integration-recheck.log` and
`work/integration-cache-recheck-2.log`, with matching scripts. Staging used one
queued batch, then sequential affected-gate reruns after it finished; no duplicate
call or measurement. The automatic commit security review failed with API 404;
that is not a successful security-review claim.

## Resume point and limits

The prepared-budget listener switch still defaults off. The accepted Task 9/10
handoff and staging harness fix are merged, and the assigned public HTTP wrappers
are implemented. Independent route-integration review remains required, including
middleware behavior on hits and the exact scope of conservative temporal bounds.
The Phase 1 meaning gate has the recorded year-zero failure; do not claim release
acceptance from the passing simulation alone. No activation, live DDL, deployment
or measurement is authorized by this checkpoint.

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
