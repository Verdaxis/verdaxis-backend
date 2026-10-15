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
  runs 94 edge comparisons over all eight owned routes. It reuses corpus rows;
  it creates no fixture IDs. The edge server uses the recorded database timezone.
  Compatibility uses a second attested test server with the existing list-group
  dispatch; it is terminated in a finally block.

## Verification checkpoint

- fmt and clippy with warnings denied: passed.
- Workspace: 112 passed.
- Disposable PostgreSQL runner: passed (119 tests at the earlier checkpoint;
  final repetition pending). Its two dedicated market matrices remain filtered
  by the existing runner.
- Map populated corpus: six cases and 94 edge comparisons pass in both KYC states.
- Python contract suite: 80 passed.
- Python contract plus all six task-named unit files: 171 passed, 11 warnings.
- Compatibility-containing `orderbook_lists` group: final result pending. Task 7b
  is absent from this lane; its failures must not be repaired here.

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

Resume: complete final repeated checks, record compatibility group differences,
then commit verification notes. PM review and merge remain separate.
