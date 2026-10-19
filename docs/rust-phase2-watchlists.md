# Phase 2 watchlists (Tasks 11 to 13)

Record of the Rust watchlist routes on `feat/rust-phase-2`. Task 11 ports the
reads W01 and W04. Tasks 12 and 13 port the container and target writes. The
policy basis is PD1 to PD4 and PD6 in the Phase 2 plan, PD4-G (decision 94) and
the plan record in `rust-phase2-plan-record.md`.

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
| Differential, both KYC states, 18 actions each | 0 | every approved pair matched |

Logs: `work/rust-phase2/task11-check-2.log` (not committed). One earlier call
ended with exit 255 when the SSH connection to the host dropped; it was rerun.
