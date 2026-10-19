# Phase 2 watchlists (Tasks 11 to 13)

Record of the Rust watchlist routes on `feat/rust-phase-2`. Task 11 ports the
reads W01 and W04. Tasks 12 and 13 port the container and target writes. The
policy basis is PD1 to PD4 and PD6 in the Phase 2 plan, PD4-G (decision 94) and
the plan record in `rust-phase2-plan-record.md`.

## Composition

`rs/read/src/crud/watchlists/http.rs` owns `router(app, settings)`, the one
composition point for the group, and `run`, which verifies the bearer and runs a
handler in the one authenticated transaction. Route registration for the
integration router is the marked block in `rs/read/src/routes/mod.rs`
(`watchlist_router`). Nothing here changes `Config` or `AppState`; the settings
(verification and the reserved PD5 cutoff) are supplied by the caller, and the
runtime secret wiring stays with Tasks 22 and 23.

Generated values (row clock and row identifiers) come from `WriteControls`.
Production requests use the real clock and random identifiers. Only a build with
the `contract-tests` feature can install a control source
(`WatchlistSettings::with_controls`), and an exhausted controlled sequence
panics instead of falling back to random identifiers.

## Task 11: reads

`rs/read/src/crud/watchlists/` holds the projection, the metrics, the batched
loader and the authenticated handlers (`read_routes`). Reads take a share lock on
the user row, never prune, refresh a snapshot or write a row.

One captured public-order relation (`capture_visible`, the Phase 1 relation and
binder) supplies both PIN membership and the active-order counts. No second
owner, provenance or KYC predicate exists in the watchlist code.

### Approved differences from live Python

| Rule | Python today | Rust |
| --- | --- | --- |
| PD4-G | Every `SLICE_BEST_PRICE_MOVED` row is visible and counted | Hidden and not counted, before and after any cutoff; the rows are kept |
| PD2 | PINs and events that name a non-public order are shown and counted | Excluded before the unread count and the latest-event time |
| PD3 | `active_order_count` counts any open, qualified order | Counts only orders in the public scope |

User-visible effect: watchlist feeds show no best-price change events until the
producer is replaced (see "New task after Task 20" in the plan record). The
PD5 cutoff stays reserved for a later trusted producer and is never a default.

Vectors: `tests/rust_contract/phase2/watchlist-approved-differences.json`
(`scenario_actions` names the action that pins each rule) and
`watchlist-reads.json` (the required cases and the action that covers each).

### Differential matrix

`watchlist-http-reads` (KYC execution gate on) and `watchlist-http-reads-gate-off`
run the same populated state through live Python and Rust on twin databases.
Every action asserts unchanged business state and no mail. An action that
differs by an approved rule names it (`PD2+PD3` joins rules) and pins both
observed values; every other response field, header and status stays strict.

| Case | Container | Result |
| --- | --- | --- |
| owner-empty, legacy-list, legacy-entries-detail | no targets; two legacy entries | equal |
| best-price-events-hidden | three best-price rows before, at and after any cutoff | PD4-G |
| public-slice-and-pin | public order with PIN and events | equal |
| hidden-order-pin-and-event | PIN and slice event on a never-public order | PD2+PD3 |
| hidden-order-event-only | visible PIN, slice event names a hidden order | PD2 |
| gate-on-order-hidden / gate-off-order-visible | owner without approved KYC | PD2+PD3 with the gate on; equal with it off |
| totals-before-truncation | 101 slices | equal (100 returned, totals of 101) |
| foreign-detail, missing-detail | other user's container; unknown id | equal 404 |
| malformed-uuid-length, -simple, -hyphenated | valid bearer, bad path id | equal 422 (FastAPI shape) |
| bad-bearer-before-uuid | invalid bearer, bad path id | equal 401 |

Fixture note: a disposable run may only hold a market-visible order through an
allowlisted DEMO organization (a database trigger fixes REAL and DEMO
provenance). The gate-dependent order is owned by a PENDING-KYC user of that
organization.

### Verification record (staging host, `heavy run 3600`)

| Gate | Own exit | Result |
| --- | ---: | --- |
| Runner and inventory Python tests | 0 | 30 passed |
| `cargo clippy -D warnings` (lib, `crud_watchlist_reads`, `watchlist_server`) | 0 | clean |
| `cargo test` (lib, `crud_watchlist_reads`) | 0 | 19 and 6 passed |
| Differential, both KYC states, 15 actions each | 0 | every approved pair matched |

Logs: `work/rust-phase2/task11-check-2.log` (not committed). One earlier call
ended with exit 255 when the SSH connection to the host dropped; it was rerun.

## Task 12: containers (W02, W03, W11)

`containers.rs` holds the three handlers. The user-row lock taken at
authentication serializes every write of one user, so radar uniqueness and the
container cap need no further locking.

- W02 selects the user's default radar and inserts one only when none exists. The
  summary is read inside a savepoint after the insert, so a failed read returns the
  database error response and the radar still commits (source contract: Python
  commits before it builds the summary).
- W03 counts every container of the user, of any kind, and refuses the eleventh
  with 400. Name length counts code points and whitespace-only names pass.
- W11 deletes events, targets and entries explicitly before the container, in one
  transaction. A failure at any step restores everything.
- Body JSON syntax is refused before authentication; authentication precedes path
  and body validation; validation errors carry no documentation `url` (FastAPI).

Scenario `watchlist-containers` runs both stacks on twin databases with the same
clock and identifier sequence. Python runs through
`tests/rust_contract/phase2/watchlist_server.py`, which patches only column
defaults and one summary call, behind the disposable-target token. Every action
asserts the exact committed change. Cases and the evidence for each are listed in
`tests/rust_contract/phase2/watchlist-containers.json`. There are no approved
differences. A concurrent pair at the container cap is not scheduled: the runner
pairs one expected status per actor, and both stacks serialize on the user row.

Not modeled: a non-JSON `Content-Type` on a body route (FastAPI then validates
raw bytes). The frontend always sends JSON.

| Gate | Own exit | Result |
| --- | ---: | --- |
| Runner and inventory Python tests | 0 | 30 passed |
| `cargo clippy -D warnings` (lib, both watchlist test files, `watchlist_server`) | 0 | clean |
| `cargo test` (lib, `crud_watchlist_reads`, `crud_watchlist_containers`) | 0 | 28, 3 and 6 passed |
| `watchlist-containers` with `--phase2-races` (29 actions, 3 barrier repeats) | 0 | every response and committed state matched |
| Read matrix after the router refactor, both KYC states | 0 | unchanged |

Logs: `work/rust-phase2/task12-check-{1,2}.log` (not committed). The first
containers run failed because the fixture gave the database-failure case no
identifier; both stacks then reported an exhausted controlled sequence, which is
the intended refusal to fall back to random identifiers.
