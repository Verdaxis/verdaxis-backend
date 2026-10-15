# Task 11 — market and voyage MCP

Lane base: `f2dd2a44`. Scope: Task 11 only. No live hosts, providers, agents,
skills, pushes, or Python application changes.

## Recorder checkpoint

- Added a bounded MCP response reader, selected only for exact `/mcp` cases.
  JSON bodies retain exact bytes. POST SSE ends at the matching JSON-RPC
  terminal response. Standalone GET records status/headers and closes the
  stream, since it has no terminal response in Python's stateless transport.
- Limits: 30 seconds total body-read time and 8 MiB. Truncated, timed-out,
  oversized, or uncorrelated POST streams fail recording. HTTP error read
  failures are not converted to empty MCP bodies.
- Red: recorder test collection failed because the new module was absent.
  Log: `work/rust-phase1/task11-recorder-red.txt`.
- Green: initial seven recorder tests passed; Python contract suite passed
  103 tests. Additional fragmentation, deadline, timeout and terminal-boundary
  tests were added. Terminal-boundary regression failed before repair.
- The populated endpoint gate ran to completion and failed as expected:
  six MCP cases, 20 differences, because the Rust MCP router is still empty.
  Log: `work/rust-phase1/task11-endpoint-red.txt`.
- The plan's obsolete `--rust-target mcp` flag is absent from the current
  runner. Used `--group mcp --seed-mode corpus`, which exercises the real
  empty endpoint through the authoritative comparator.

## Checkpoint checks

- Rust fmt and clippy (`--workspace --all-targets -- -D warnings`): passed.
- Rust workspace: **125 passed**.
- Disposable PostgreSQL runner: **133 passed**, two migrated market matrix
  tests filtered by that runner for their separate fixture runner.
- Python contract suite: **107 passed**, including 11 MCP recorder tests.
- Python MCP server, voyage engine and voyage prices: **90 passed**.
- Populated MCP group: **failed**, six cases and 20 difference paths; Rust
  endpoint remains unimplemented. No golden was published.
- No tests remain running. Logs: `work/rust-phase1/task11-*.txt`.

## rmcp and oracle checkpoint (continuation)

- Added the rmcp 3.5.1 stateless JSON HTTP service with generated catalog and
  slice tool dispatch. It preserves the existing no-argument router seam.
  Production requests return 404; exact Host and any-Origin guards run first.
- Added anonymous slice projection using `OrderSurface::Orderbook`, the shared
  binder/captured clock, exact decimal addition/comparison, and Task 10b's
  benchmark loader/fallback. No shared SQL changed and no product literal was
  added to executable Rust.
- Four offline Rust MCP tests pass, including saturation of all eight slots:
  initialize/list-tools/catalog bypass admission; slice reports busy. Slice
  and transport tests failed to compile before their implementation.
- Exported sea distances, fuel properties and all three Python tool schemas
  as data with `scripts/rust_contract/export_voyage.py`.
- Expanded the MCP group to 29 cases. Added a failing recorder regression for
  dropped request bodies, then retained exact MCP request bodies in golden.
- `record_mcp.py` reused disposable runner ownership and recorded 29 cases
  twice in each KYC state, stable in both. Golden source is the actual Python
  oracle HEAD `1fb9f26d87588b827b294d62230f46eef60a762a`; application Python is
  unchanged. There are 58 recorded case directories, each with tool schemas.
- Expanded differential gate: **73 differences**. Four successful slice
  structured payloads already match; text formatting/envelope ordering still
  differ. Python prefixes tool errors with `Error executing tool <name>:` and
  uses Python repr in unsupported identity messages. These remain repairs.
- Workspace **129 passed**, clippy passed. Python contract suite **92 passed,
  16 failed**: all failures are the build-owned policy exporter's assumption
  that exactly one SHA directory exists. Decision 19 requires this separate
  MCP oracle SHA. Do not relabel the MCP bytes or edit other groups to hide it;
  integration must select the catalog oracle independently of MCP recordings.

### Exact next resume point

The rmcp mount currently registers **two**, not three tools. Implement
`plan_voyage_fuel` plus its typed validation, lookup resolver and exact
28-significant-digit Decimal arithmetic. Add failing oracle vectors first.
Then use the recorded schemas to adapt SDK output and protocol error envelopes
without replacing rmcp dispatch. Add per-tool errors, recursive privacy scan,
real SQL cancellation/capacity tests, and the missing voyage boundary cases.
The present mount is a checkpoint, not a parity-complete implementation.
`work/rust-phase1/task11-expanded-red.txt` records the 73 current differences;
`task11-golden-record.txt` records both stable oracle passes. All runs finished.

### Checks after commits `6a58e439` and `668996d3`

- fmt passed; workspace **129 passed**; clippy passed.
- Disposable PostgreSQL runner **137 passed**; two matrix tests remain filtered
  by its existing dedicated-runner split.
- Python MCP/voyage suites **90 passed**.
- Current populated MCP differential gate: **67 differences**, down from 73
  after matching success text formatting. This is still a failed gate.
- Python contract suite: **92 passed, 16 failed**, all the catalog-oracle
  directory-selection issue described above. No comparator was weakened.
- Not run because not implemented: Rust voyage decimal vectors and full
  three-tool cancellation/privacy/error acceptance. No test process remains.

## Single-golden correction and Decimal checkpoint

The user's later direction supersedes the separate committed MCP SHA layout.
Commit `c6bdd53c` moves all 58 MCP recordings to untracked
`work/rust-phase1/mcp-evidence/golden/1fb9f26d87588b827b294d62230f46eef60a762a/`.
`record_mcp.py` now writes only beneath that evidence root. The PM records MCP
into the single merged golden after lane merge. Contract tests are green again.

Added `mcp/decimal.rs`: signed arbitrary-precision integer coefficients with
28-significant-digit half-even operations and half-up output quantization.
The dependency correction adds `num-bigint` and `num-traits` (and the resolved
`num-integer` transitive dependency); no float arithmetic or tolerance changes.
A reproducible Python exporter produces **938 exact operation/output vectors**,
including signed zero, tiny decimals, thirds and rounding boundaries. The Rust
test failed before implementation and now passes all vectors. The adapter is
not yet a full voyage engine; its input exponent bound needs review against the
final typed request domain before acceptance.

The protocol adapter now preserves Python envelope field order without sorting
structured payloads, uses recorded schemas only for tools actually registered,
retains pretty text output, and prefixes slice errors as Python does. Initialize
metadata and Origin header absence were repaired. rmcp still owns dispatch.
Current populated differential gate: **41 differences**, not zero. Success
catalog/slice cases now match. Full voyage, transport errors and validation
remain incomplete. The gate compared fresh Python/Rust on one disposable DB;
a saved lane-evidence replay command remains to be added.

Latest checks: fmt/clippy pass; workspace **131 passed**; disposable PostgreSQL
**139 passed** (two matrix tests filtered by its existing split); Python contract
suite **110 passed**; Python MCP/voyage **90 passed**. No test remains running.
Logs: `work/rust-phase1/task11-current-*.txt`.

**Resume here:** implement `mcp/voyage.rs` and typed request validation using the
new Decimal adapter, then the batched voyage ASK/benchmark resolver and third
rmcp tool. Add full-engine oracle vectors before implementation. Finish the
41 protocol/validation differences, three-tool privacy/error/cancellation
coverage and lane-evidence replay. Do not claim Task 11 complete at this point.

## Pure-engine checkpoint `0f481b27`

Added full-engine oracle tests first; the test failed because `mcp::voyage`
was absent (`task11-voyage-red.txt`). Ten full result/error cases now match
Python exactly: golden blends, missing assumptions, conventional-only,
green-only, missing intensity, no EU scope, old/future starts, duration and
invalid Cape route. `export_voyage_vectors.py` regenerates these fixtures.

All three rmcp tools now register. The voyage tool uses `run_admitted` for
its complete lookup/calculation unit. Price candidates use the shared
Orderbook relation and bind helpers, with one batched ASK read ordered by
price/acceptance ordinal; Task 10b supplies benchmark reads/fallback data.
Both database tools return busy while initialize/list-tools/catalog bypass
all eight occupied slots. No existing shared SQL or Python app was changed.

The engine has typed serde request structures and numeric/length bounds,
but **validation parity is not complete**: port canonicalization, detailed
Pydantic errors and some field semantics need work. The pure engine also
needs broader rounding/annual-boundary vectors and an explicit catalog-lane
check for caller-supplied price maps. Current vectors do not prove those.

Latest checks: fmt (after correcting test formatting) and clippy pass;
workspace **132 passed**; PostgreSQL **140 passed**, two matrix tests filtered;
Python contracts **110 passed**; Python MCP/voyage **90 passed**. MCP corpus
has **29 differences**, not zero. The first disposable attempt failed database
startup; cleanup ran, and the retry completed. Logs are `task11-engine-*` and
`task11-voyage-contract.txt` under `work/rust-phase1/`. No tests remain running.

**Next resume:** fix remaining recorded protocol differences (tool-list byte
order, malformed/unknown requests, GET stream, DELETE, notification headers,
unsupported versions), Python repr and Pydantic validation errors. Add lane-
evidence replay, three-tool recursive privacy/error/cancellation tests and
expanded voyage fixtures, including green market-price lookup differential
cases. Preserve the single committed golden directory. This is not acceptance.

## Zero-difference corpus and saved-evidence replay checkpoint

- `7f878660`: transport/schema compatibility, with failing schema-order and
  stateless transport tests before repair. GET remains an open header-first
  stream; recorder closes it deterministically. rmcp owns normal dispatch.
- `da958ba9`: recorded validation errors, Python string repr and port-name
  canonicalization, plus three green voyage cases. The corpus is now 32 cases.
- Fresh differential **32/32 matched in both KYC states**, and saved lane-wire
  replay **32/32 matched in both states**. No comparator changes or source
  nondeterminism were found in these cases.
- Evidence: `work/rust-phase1/mcp-evidence/golden/da958ba9dbc60e6285f2de4e0790bfa2b47dc7bd/`;
  each KYC state was stable across two recordings. It remains untracked.
- Replay initially failed on 16 schema numeric types: diagnostic response.json
  stringifies Decimal. The replay now reconstructs the expected exchange from
  retained response.body through authoritative `capture`, and verifies the
  stored body digest. This fixes the reader, not the comparison rules.
- The new feature-gated `rs/contract/tests/mcp.rs` runs normal differential
  comparison and, when `MCP_LANE_EVIDENCE` is supplied, the saved-wire replay.
- Checks: fmt/clippy passed; workspace **134 passed**; disposable PostgreSQL
  **142 passed** (two matrix tests filtered); Python contracts **110 passed**;
  Python MCP/voyage **90 passed**. All test runs finished.

**Still not full Task 11 acceptance.** Zero differences cover the current
32 cases, not every Pydantic input or transport state. Next add the complete
validation-domain tests/implementation (wrong types, bounds, extras, date
coercion, nested error accumulation and truncation), three-tool recursive
privacy/error-class tests, and real MCP SQL cancellation/capacity proofs.
Current validation adapter covers missing top-level tool arguments and port
errors only; other validation still comes from serde/engine and is not Python-
message-compatible. Modern per-request metadata handling is only the recorded
missing-envelope branch. Add adversarial body/version cases before expanding
it. Full pure-engine caller-price lane checks and annual/rounding boundaries
also remain. Resume with these acceptance gaps, not another transport rewrite.

## Real SQL cancellation and 44-case checkpoint

Commit `7417d2c4` expands the corpus to 44 cases. New cases cover numeric bounds,
extra vessel fields, null vessel, empty legs, inclusive start bounds, invalid
Cape routes, overlong voyages, 2035 targets and lowercase port normalization.
The initial expanded gate failed on six validation messages (12 paths); those
recorded diagnostics now match. This is additional coverage, not proof of the
complete Pydantic input domain.

`mcp_acceptance.py` runs after differential comparison on the attested disposable
server. It recursively scans both text and structured successful outputs for
private field names across all three tools. For each database tool it holds a
real ACCESS EXCLUSIVE order-table lock, waits for eight blocked SQL statements,
cancels their HTTP clients, verifies all eight statements remain active, checks
that a ninth MCP database request is busy while catalog succeeds, releases the
lock and verifies recovery. The first attempt raced cleanup between batches;
the final probe waits for prior SQL to drain before the next batch. Both tool
proofs pass in both KYC states. Closed-pool offline tests separately verify
sanitized database-unavailable errors for both tools and catalog availability.

- Fresh differential and saved-wire replay: **44/44 matched in both KYC states**.
- Oracle recordings: two stable passes per state, under untracked evidence SHA
  `7417d2c4` (full SHA directory under `work/rust-phase1/mcp-evidence/golden/`).
- fmt/clippy passed; workspace **135 passed**; PostgreSQL **143 passed** (two
  matrix tests filtered); Python contract **110 passed**; Python MCP/voyage
  **90 passed**. All test runs finished. Logs: `task11-accept-*.txt`.

**Next resume:** finish the general Pydantic validation adapter (wrong scalar
and container types, absent nested fields, decimal errors, date coercion,
unknown/duplicate fuels, all extra-field locations, aggregate error ordering
and truncation). Add unexpected-error classification tests rather than treating
all internal errors as database unavailable. Strengthen privacy with actual
fixture identifier sentinels, and expand pure caller-price lane and rounding
boundaries. The current 44-case gate is green, but these untested domains remain
acceptance gaps; Task 11 is still not declared complete. No nondeterminism or
comparator relaxation occurred. Single committed golden remains untouched.

## Remaining implementation and acceptance

Task 11 is **not complete**. Implement the rmcp 3.5.1 adapter, all three tools,
source-derived voyage data, typed validation, exact Decimal engine, shared
market price resolution, and error/privacy/admission tests. Record the MCP
oracle twice in both KYC states under its actual source SHA. Do not merge the
build branch's golden here. Use `DB_SESSION_TIME_ZONE=Asia/Singapore` for this
oracle and `run_admitted` for database tools only.

The current no-argument `mcp_router() -> Router<AppState>` is called by the
shared contract server. Preserve that integration seam without editing the
build-owned dispatcher. Task 13 owns production composition. Task 11's plan
text saying two slots is superseded by decision 11 and the task brief: use the
existing eight-slot gate, not a new semaphore.

Shared market relations: use `OrderSurface::Orderbook` for voyage public ASK
and slice reads, not the benchmark-only full typed filter. Python's
`voyage_prices.py` uses `public_order_scope` directly. Reuse Task 10b's shared
benchmark projection; do not substitute the canonical curve slice, whose
provenance selection differs from the MCP snapshot. Any new SQL belongs in a
new purpose-named shared file with comparison tests.
