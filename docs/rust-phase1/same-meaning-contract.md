# Phase 1 same-meaning successor contract

## Authority and release boundary

This branch prepares Tasks 1–6 only. It does not authorize deployment or change
`integrate/rust-phase-1-20261018`, the first byte gate, Caddy activation, or its
artifact. Production activation requires Phase 1 live in production and the
founder's decision. Production Phase 1 identity is **not verified** by this lane.
No live host or health endpoint was inspected.

No load, latency, memory, capacity, or rate measurement is authorized in Tasks
1–6 (decision 96). Do not refresh Python's decision-87 baseline.

## Source checkpoints

| Source | Exact identity | Scope |
| --- | --- | --- |
| Integrated base | `34dacf327d56ec32ddaf74c80390986a4fe1061d` | Starting source, not deployment proof |
| Final shared SQL speed handoff | `738fe0e85d85d7667263e8950fe4de4c05831e38` | Already integrated; do not repeat the SQL edits |
| Integrated gate successor | `3990151f04c99232a8ea03592828240a682e280c` | Includes decision-96 rate-limited sample-floor formatting |
| Frozen-baseline gate handoff | `92a97587d72678a75921734b09d1d93727884059` | Already integrated; reuse baseline and gate implementation |
| Reviewed Phase 2 comparator | `64179cd15c85e1da76ec49d8756b6f7419e94c66` | Import by Git history, then extend the same engine |
| Observed Phase 2 remote tip | `a446c628c4dabe486b157217fc16b60570109242` | Source observation only; do not merge unrelated Phase 2 work |
| Frontend step 5b | merge `c3633f70`, reviewed tip `03248101` | Tracker records PR 93 and production promotion 39080087535; full frontend SHA needs reconciliation before release |
| Live Phase 1 identity | Not established | Blocks activation, not isolated preparation |

The frontend identity above is a historical tracker observation, not a new live
check. Do not turn a short SHA into a fabricated full identity.

## Inventory and default policy

`tests/rust_contract/phase1_meaning/routes.json` pins 27 measured GET routes,
three MCP tools, and all 487 requests across the 11 existing correctness groups.
It copies whole request definitions, not only positive request IDs. This retains
malformed inputs, protocol versions, methods, redirects, supplied headers, policy
fixtures, and negative cases. `test_phase1_meaning_inventory.py` checks completeness
against the existing corpus and tests missing and changed coverage.

Comparator IDs use `P1-` prefixes because Phase 2 already uses R01–R05 for referral
routes. These are distinct route namespaces in the same comparator, not a second
comparison engine. Task 2 must add an immutable Phase 1 ledger through the existing
loader. The byte path and `phase2-meaning-v1` remain unchanged.

Until Task 2 reviews a scalar path, comparison is strict. Empty scalar path lists
are not permission to normalize every numeric-looking string. Unclassified cases
are strict and uncached. CSV exports remain byte-exact without an approved codec.

### Accepted differences

- Decision 84: only declared MCP validation diagnostic presentation may change.
  Rejection, semantic error kind, location, input, envelope and `isError` remain
  strict. Ordinary messages remain exact.
- Decision 85: tape membership uses `(confirmed_at DESC, full trade UUID DESC)`
  **before** pagination. The expected page comes from the complete pinned Python
  filtered relation and public DTO projection. Never sort a Python page or compare
  only truncated public IDs. Canonical tape array order is strict.
- JSON syntax and exact mathematical JSON-number equivalence are permitted only
  through the shared reviewed profile. Decimal strings and timestamps need exact
  declared paths. Types, null/absence, values, privacy, status and meaningful
  headers remain strict. News permits only equal-`published_at` tie permutations;
  price summaries retain their reviewed collection rule.
- Decision 94: do not fix Python. The Phase 2 plan section 0 owns the known-defect
  list. Its PD1–PD7 concern later authenticated routes, not a blanket Phase 1
  exception. Any applicable defect must be an explicit approved difference, never
  a silent pass. New defects require PM approval.

## Dependency handoff for later cache work

Reference caching is allowed only for R01–R05: ports/intelligence, catalog products,
delivery points, and published news. Use complete typed query keys, environment
and policy identity, a monotonic 60-second expiry, and byte/entry bounds. Cache only
successful public payloads; rebuild request headers. Invalid input and errors do
not enter the cache. No personal, auth, health, or private data may be cached.

Trading routes remain uncached in Tasks 1–6. Their shared dependency families are
orders, formal trade snapshots and state, product/lane eligibility, delivery-point
activity, user and organization admission/provenance, assisted-member eligibility,
KYC reviews and grace, expiry and the captured clock, quarantine, trusted signals,
and live/manual benchmarks. Task 7 must derive the exact table/column trigger
matrix from source; this family inventory does not authorize infrastructure or
claim that economic events cover policy changes. Python remains the HTTP writer
and SSE sequencer.

## Frozen future sampling schedule

`phase1-meaning-sampling-v2` is a future qualification schedule, not permission to
run it now. Light IDs: R01–R05, R07, R08, `list_markets`, `plan_voyage_fuel`.
Heavy IDs: R06, R09–R27, `get_market_slice`. Use equal-weight rounds within each
class; keep baseline verdict weights and query identity unchanged.

| Size/class | Candidate RPS | Window per cache mode | Chunks |
| --- | ---: | ---: | --- |
| Small/heavy | 10 | 2,100 s | 2 × 1,050 s |
| Large/heavy | 2 | 10,500 s | 5 × 2,100 s |
| Small/light | 40 | 225 s | 2 × 112.5 s |
| Large/light | 40 | 225 s | 2 × 112.5 s |

Future qualification needs 1,000 successful samples per ID/size/cache mode, exact
identity and complete-round ranges, separate off/on cells, and Rust-only timing.
Candidate and 25%-higher guard probes follow 60 s warm-up and run for 120 s each.
Ladders are small heavy `[10,5,2,1,0.5,0.25]`, large heavy
`[2,1,0.5,0.25,0.1,0.05]`, and light `[40,20,10,5]`. Lower rates preserve counts
and extend durations; heavy subchunks are at most 4,200 measured seconds. No
Python performance cell may be collected. Task 11 needs separate PM authority.

## Shared-file ownership and resume point

Execute Tasks 1–6 serially on this branch, without subagents. Task 4 owns amount
call replacements in tape/prices/orderbook-map until its commit. Task 5 takes tape
after that handoff. Later SQL lanes and cache integration must use the committed
amount API; shared visibility patches apply serially, Task 9 then Task 10.
Tasks 7–12 are outside this brief.

### Task 1 checkpoint

- Inventory RED: pytest exit `1`, wrapper exit `1`; six expected failures because
  the successor inventory did not exist.
- Inventory GREEN plus existing inventory suite: pytest exit `0`, wrapper exit
  `0`; 10 passed on the staging test host with Python 3.12.
- `git diff --check`: exit `0`.
- No build, test, or project code ran on the Mac. No measurement ran anywhere.
- CI: not run; no push or PR creation while publication authority is unresolved.
- Remaining inventory detail: Task 2 must replace the strict scalar placeholders
  with source-derived decimal/timestamp paths in the shared ledger and reconcile
  those paths back into this inventory. This checkpoint does not claim typed-path
  normalization is implemented.

Next: reuse the reviewed Phase 2 comparator through Git history for Task 2; do not
copy its files or introduce another normalizer. Publication is pending clarification
because the brief both requests a draft PR and forbids lane pushes.

Before continuing, resolve whether this lane may push its own branch and create
the requested draft PR. No run remains active at this checkpoint. Tasks 2–6 have
not started; Tasks 7–12 remain outside the brief.
