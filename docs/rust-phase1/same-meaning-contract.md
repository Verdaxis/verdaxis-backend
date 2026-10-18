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

PM clarified publication authority: push only `feat/rust-same-meaning` and open
the draft PR. PR 90 targets staging, is draft, and says Do not merge. Task 1 CI
completed: offline, contract, test, postgres-phase0 and postgres-analytics passed;
publish skipped. No PR comments, edits, merge or close actions are authorized.

### Task 2 checkpoint

- Reused `64179cd1` through cherry-pick with source attribution (`6c44e31a`).
  Kept the already-integrated baseline architecture notes and omitted unrelated
  Phase 2 output documentation. No application Python code changed.
- Imported comparator regression suite: 94 passed, test exit `0`, wrapper exit `0`.
- RED: 23 failed, 64 passed; test exit `1`, wrapper exit `1` (missing successor
  ledger/profile). Source-derived ledger export: exit `0`, wrapper exit `0`.
- First GREEN: 1 failed, 122 passed, exit `1`; the negative `isError=false` mutation
  was correctly rejected as invalid inner JSON, while the test expected a normal
  difference. The test now requires invalid evidence for that malformed success.
- Final GREEN: 123 passed, test exit `0`, wrapper exit `0`. One existing Pydantic
  class-config warning. Includes unchanged Phase 2, byte, news and tape checks.
- Immutable Phase 1 ledger digest:
  `92578257e18b80120fbb87d27d3a4f3ae6e83bc84342ba33bd07aed20d38b80d`.
  Inventory scalar placeholders are replaced with the reviewed paths.
- The engine remains `phase2_meaning.py`; one profile dispatch, one JSON walker,
  typed-map traversal only at reviewed paths, and no new comparator or repr parser.
  Decision 84 validates structured error semantics against the request validator.
- CSV and non-JSON auxiliary bytes stay strict. Tape membership and order stay
  strict; Task 5 must supply the canonical complete-relation page oracle.
- No measurements ran. Next: Task 3 must reuse the existing decision-87 baseline
  and gate, not create another baseline file or refresh Python performance.

### Task 3 checkpoint

- Reused the accepted decision-87 baseline at its existing path. Its bytes remain
  SHA-256 `3c74f7960add211757198149241f1633ef90a165ec89a902ac40bf79c1f8ea5e`.
  No duplicate baseline file or fresh Python measurement was created. Historical
  source provenance remains PM-attested; this lane did not re-open its private raw
  source on the production test host.
- Successor policy is explicit: `frozen_verdict(..., successor=True)` and Rust
  `meaning_performance_gate`. Existing first-cutover defaults retain their policy.
  Successor error rates block at both sizes, exact cross-products allow unequal
  offered totals, positive lower rates are valid, transport failures block, and
  the full sample floor remains required. Large speed remains a fix list.
- Python RED: 9 failed, 30 passed; test/wrapper exit `1`. GREEN with measurement
  unit tests: 131 passed; test/wrapper exit `0`. No measurement process started.
- Rust RED: missing successor API, cargo/wrapper exit `101`. GREEN: 38 tests
  passed across meaning, frozen and legacy gate suites; cargo/wrapper exit `0`.
- Initial fmt command lacked `--all` (exit `1`). Corrected check found import
  ordering differences (exit `1`); host formatting and final check exit `0`.
- Successor bundle requires digest-checked both-size correctness artifacts and
  actual JUnit case coverage; caller booleans cannot qualify a release. Known
  failures take precedence over incomplete evidence. Task 6 must produce the
  declared correctness artifact fields and canonical tape proof.
- Task 2 CI completed: all five required jobs passed; publish skipped.
- Skipped measurements: all latency, memory, CPU, rate/capacity and probes under
  decision 96. No release verdict or production permission is claimed.

### Task 4 partial checkpoint — blocked, not accepted

Task 3 is committed as `5d3704ca`. Its CI completed with all five required jobs
passing and publication skipped. Task 4 edits remain uncommitted and unpushed.

- Added a shared arbitrary-precision `Amount` implementation and domain product/sum
  operations, moved voyage/validation consumers to it, and replaced map-specific
  digit arithmetic. Retained 28-digit intermediate economic rounding and separate
  digest canonicalization. This is not yet the complete Task 4 cleanup.
- Amount RED: cargo/wrapper `101` (missing amount module). First GREEN: `101`
  (test expected an inconsistent simple spelling). Second GREEN: `101` (old voyage
  string equality rejected value-equivalent distance spelling). After routing
  those assertions through the shared Python comparator, the amount, market,
  price and voyage suites passed 33 tests; cargo/wrapper `0`.
- Structured tool-validation diagnostics now preserve kind/location/message/input
  as JSON. Protocol diagnostics are still exact and have not been removed.
  Existing diagnostic spelling assertions failed as expected (`101`). A semantic
  diagnostic bridge passed rejected-input comparisons but still failed accepted
  normalized-string equality (`101`). The next edit moved positive normalization
  checks to the pinned Python model; that edit is **not verified**.
- Last call: `work/task4-diagnostic-final.log`, `CALL_EXIT=255`, no
  `DIAGNOSTIC_GREEN_EXIT` marker. Compilation finished; the test did not report a
  result before the connection ended. Do not call this a test pass or infer a
  test exit from the wrapper. No call was interrupted and no extra host call ran
  concurrently from this lane.
- A subsequent light read-only process check showed no lane systemd unit,
  `mcp_validation`, `compare_values`, or `heavy run` process remaining.
- No measurements or live application changes ran. No Python application source
  was changed. Do not push the partial Task 4 tree.

Resume: diagnose why the MCP positive-normalization bridge stalled (including
extreme-exponent Decimal comparisons and process lifecycle), add bounded failure
handling, and rerun it through the host helper without increasing its time limit.
Then finish Task 4: remove obsolete amount module/format helpers only after all
consumers move; remove envelope ordering; preserve protocol-version fields and
non-approved errors; verify all semantic vectors, shared comparator tests, fmt and
CI. Tasks 5–6 have not started. Tasks 7–12 remain outside this brief.
