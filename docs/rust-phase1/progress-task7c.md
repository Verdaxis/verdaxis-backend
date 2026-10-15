# Task 7c — Public map and compatibility projections

Base: `77842676`, branch `feat/rust-phase-1-lane-7c`. Scope: R19–R25/R27.
No Python feature changes, shared visibility changes, policy changes, live access,
skills, agents, push, or deployment.

## Implementation

- All reads use `run_admitted`, `OrderQuery`, the shared binder, and one captured
  instant. Maps/counts/distinct use `Orderbook`; compatibility uses `Listings`.
- Five SQL tails group, count, rank recent asks, select distinct labels, and page
  listings only after eligibility. Aggregate product totals precede the limit.
- Compact maps retain separate REAL/DEMO totals over all windows, spot prices,
  and the nearest demo window. Latest asks use created time descending then ID
  descending, with output ordered by point name. Listings use ascending ID ties.
- Zero-filled counts preserve catalog dictionary order on the wire. The required
  `product_counts` helper returns a BTreeMap; that map does not set wire order.
- Compatibility has its own explicit public allowlist. It uses the legacy full
  declaration validation before redaction, not the snapshot's public-only schema
  validation. This distinction is required by `inventory._listing_payload`.
- New shared files: `market/map_decimal.rs` for exact nonnegative NUMERIC comparison
  and Python-context addition; `market/listing_terms.rs` for schema ordering using
  Task 7a's recorded field lists. Route-local path modules avoid changes to
  integration-owned module registration. No new shared SQL relation was needed.

## Test-first evidence

Logs are uncommitted under `work/rust-phase1/task7c-*.log`.

- `red-unit`: missing `product_counts` import; `red-corpus`: populated group fails
  against the empty router. Both ran before implementation.
- `edges`: listings exact body hash failed because nested term key order differed.
  The schema-order projection fixes it without changing the comparator.
- `red-decimal`: compact reduction failed on values outside rust_decimal's range.
  Exact arithmetic fixes the failure. Python comparison then exposed positive
  exponent scale loss; signed scales fix it. Timestamp comparison now parses
  instants rather than comparing RFC3339 strings with optional fractional parts.
- Pure tests cover zero-filled counts, separate evidence, all-window totals,
  nearest forward demo sides, decimal boundaries and microsecond observation order.
  Seven arithmetic vectors compare directly with CPython Decimal.
- The map contract target compares the unchanged populated six-case group, then
  runs 96 edge/corpus comparisons over all eight owned routes, including the two
  compatibility cases from the list group and its malformed-history fixture. It reuses corpus rows;
  it creates no fixture IDs. The edge server uses the recorded database timezone.
  Compatibility uses a second attested test server with the existing list-group
  dispatch; it is terminated in a finally block.

## Verification checkpoint

- fmt and clippy with warnings denied: passed.
- Workspace: 112 passed.
- Disposable PostgreSQL runner: 120 passed on final repetition. Its two dedicated
  market matrices remain filtered by the existing runner.
- Map populated corpus: six cases and 96 edge/corpus comparisons pass in both KYC states.
- Python contract suite: 95 passed, five warnings.
- All six task-named Python unit files: 76 passed, 11 warnings.
- Combined Python contract and task-named unit run: 171 passed, 11 warnings.
- Compatibility-containing `orderbook_lists` group: 39 differences across 13 cases,
  all absent Task 7b routes or R19's integration-owned dispatch gap. Both R27
  cases match, including malformed private history. The task-local R19 comparisons
  pass. No unrelated implementation or dispatch was changed.
- The full Python unit suite and dedicated shared-market matrix runner were not
  repeated: this task changes neither Python nor shared eligibility. All requested
  Task 7c checks ran. Other sessions' Docker containers were left untouched.

## Deviations and resume point

- The runner has no `--rust-target` option. It discovers the task-local contract
  target by group. No harness, manifest, golden, or case inventory edit was needed.
- Added the task-local contract target and Python edge probe, as Task 7a did.
- `ARCHITECTURE.md` remains integration-owned under the lane allowlist. Integration
  should record these eight projections and their two new shared helper files.
- No typed execution predicate is added to aggregate/map visibility. Python uses
  shallow public eligibility here; benchmark qualification is a separate contract.
- Task 13 owns routing assembly, slash/method integration and private-route routing.
  The shared server currently places R19/R27 cases in `orderbook_lists`, but mounts
  aggregate only in the map group. Task-local comparisons cover both without
  changing that integration-owned dispatch.

## Final commits and resume

- `2fa7fe2a`: public map and compatibility reads, SQL tails and regression tests.
- `6dc1baa7`: exact legacy serializer failure envelope and malformed-history probe.
- The following documentation-only commit records final verification.

The compatibility repair was test-first: the full list-group corpus exposed four
owned differences in `T4B-negative-fame-holder`. The existing unhandled-server-error
seam preserves Python's plain-text 500 and missing CORS/request-ID headers. After
repair, those four differences disappear; 39 unrelated dispatch/list differences
remain. Both map runs include explicit legacy missing-declaration visibility and
public identity-omission assertions. No corpus normalization was added.

Resume: PM review and merge Task 7c. Task 13 must assemble routing and resolve the
recorded dispatch gap. Task 7b, Phase 0, architecture integration and deployment
remain separate work. No further implementation is pending in this lane.
