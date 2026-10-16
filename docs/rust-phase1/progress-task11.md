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

## Independent-review repair checkpoint — 2026-10-16

- `a7a50a9a` repairs **M1** and **M4**. Price-map keys use Python whitespace
  and catalog case-fold equivalence, preserving the last canonical-key value.
  Both lookup generation and pure-engine price consumption use one catalog-lane
  predicate. Two new Rust tests failed first, then passed (three voyage tests
  total), including green-only forbidden-lane input.
- `99fdaa8a` repairs **N2**. Eight scanner tests failed first, then passed.
  The acceptance gate reads actual private fixture IDs and declaration values
  from the attested disposable database and checks both text and structured
  output, including values under harmless keys. Public catalog IDs are excluded.
- MCP now has **47 cases**: padded/case-folded price keys, canonical-key collision,
  and a forbidden departure lane were added. A caller-supplied forbidden price
  is a pure-engine input, not an HTTP tool argument; its direct regression is in
  `rs/read/tests/voyage.rs`.
- Two stable Python recordings per KYC state are under untracked
  `work/rust-phase1/mcp-evidence/golden/ba5a5f57ceac899e2c098055902d89925c937abb/`.
  That is the actual unchanged Python source HEAD at recording time. No committed
  golden file changed. No Python source nondeterminism was found.
- Fresh differential and saved-wire replay: **47/47 matched in both KYC states**.
  Hidden-value privacy scans and both SQL cancellation proofs passed in both.
- Checkpoint checks: fmt and clippy passed; offline Rust **137 passed**;
  PostgreSQL **145 passed** (the existing two matrix filters remain); Python
  contract tests **118 passed**. Logs: `work/rust-phase1/task11-fix-*.log`.

### Exact repair resume point

**Current status supersedes the historical checkpoints below.** M3's reviewed
invalid-envelope and modern rejection cases are implemented in `34404585`;
N1 typed failure classification is implemented in `485a5d1b`. Both had failing
regressions first. `2007fd60` repairs M2 tuple error ordering and adjusted-exponent
constructor overflow; the numeric scientific-year case already matched and is
now retained. `a56b669f` attests protocol fixtures against the Python SDK.
The current corpus is 136 MCP cases. Shared port authority and the requested
modern negotiation cases are complete with both KYC wire gates green; see
**Shared port authority and modern negotiation** below for current evidence and
integration handoff. Historical blockers in this file are superseded there.
Final Task 11 acceptance remains the PM/independent review decision.
`c384156b` adds a failing-first 35-vector Python validation matrix and a
schema-driven validator. It covers unknown intensity keys, midnight dates,
padded Decimals, absent/wrong nested fields, superscript IMO rejection, the
review's tiny exponent, nested extras, custom model/map checks and aggregate
error ordering/truncation. All invalid matrix cases also run under saturated
admission. The pure engine now shares normalization. The MCP corpus has 82 cases.

`38dd0874` adds 17 failing-first vectors for integer float/string/bool coercion,
integer diagnostics, Unicode decimal/IMO digits, and SDK JSON-string request
pre-parsing. The matrix now has 52 vectors; MCP has 99 cases. Python's Unicode
Nd zero ranges are exported as data and checked against the oracle. Existing
saturation tests consume the expanded invalid matrix. This closes these concrete
coercion cases, not all of M2. The expanded matrix has 39 rejected and 13 accepted
inputs. Checks for this checkpoint: fmt/clippy pass, offline workspace **139**,
PostgreSQL **147**, Python contracts **120**, combined Python oracle/contracts
**221** passed. Both KYC recordings are stable across two passes at fixed source
`38dd087478a5ddc4bbebe9fae370b1ebad8c272f`, 99 cases per state. Logs are
`work/rust-phase1/task11-coercion-*.log`. No source nondeterminism was found.
Fresh differential and saved-wire replay **99/99 passed in both KYC states**;
hidden-value privacy and both SQL cancellation proofs passed. All test runs
finished before this checkpoint. The single committed golden remains untouched.

`8dde3cbb` repairs the concrete wide-exponent/underflow gap: i64 constructor
exponents, magnitude-based comparisons, bounded half-even subnormal rounding,
zero exponent clamping and bounded output quantization. Wide operation vectors
failed first; a separate exact-cancellation vector exposed an early-rounding
regression, then passed after preserving close opposite-sign operands. Three
full-voyage underflow vectors and six wire constructor-bound cases were added.
The matrix now has 58 inputs and MCP has 105 cases. Additional independent
probes compared 4,800 bounded random operations and 1,600 subnormal operations
with Python: zero differences. Initial exporter execution failed because a
formatter removed an unused import before its use was added; the import was
restored before generating the red vectors. Checkpoint checks: fmt/clippy pass;
offline workspace **139**, PostgreSQL **147**, Python contracts **120**, combined
Python oracle/contracts **221** passed. The arithmetic corpus has **984 vectors**;
full-voyage corpus has **13 vectors**. Both KYC recordings have 105 cases stable
across two passes at source `8dde3cbb9376dac499ffa5ff75ab56d41ae2f6ac`.
Fresh differential and saved-wire replay **105/105 passed in both KYC states**;
privacy and both SQL cancellation proofs passed. All runs finished. No source
nondeterminism was found and no committed golden changed.
Logs: `work/rust-phase1/task11-wide-*.log`.

**Continue M2, not M3 yet:** add failing vectors for wrong root argument types,
tuple maximum/error ordering and scientific-notation integer numeric cases.
Confirmed remaining tuple rule: seven invalid fuel names produce seven enum
errors plus `too_short`, but no `too_long`; the current adapter checks raw length.
Also add constructor adjusted-exponent overflow for multi-digit coefficients:
Python rejects `10e999999999999999999`, while the current parser only checks the
stored exponent. Verify near-context overflow/error classification and
long-coefficient cancellation.
The current wide repair covers the tested constructor bounds and underflow, not
every possible Decimal operation exposed by the public arithmetic helper.
The schema validator is deliberately not claimed as a general Pydantic engine.
Keep source-derived schemas and do not add runtime Python calls or fixture lookups.
Then fix M3's invalid JSON-RPC version before tool validation and empty/partial
modern `_meta` envelopes. N1 needs a typed failure classification through the
shared admission seam: currently join/panic and database errors both become
`ApiError::internal()`. Do not relabel all 500s or change REST wire errors.

The M2 checkpoint has 35 oracle vectors (31 rejected, four accepted), plus
pre-admission saturation checks for all 31 rejected inputs. Offline workspace
**139 passed**, PostgreSQL **147 passed** (existing matrix filters), Python
contracts **119 passed**; fmt/clippy passed. The first recording crossed a Git
commit between KYC states, so off-state replay failed with `FileNotFoundError`
for the old SHA's missing evidence. This was a recorder invocation error, not
Python nondeterminism. Never commit while `record_mcp` is running: it constructs
a fresh runner/source SHA for each state. Fixed-HEAD recordings now contain 82
stable cases in each KYC state beneath source
`c384156b194a1b0db66d750f56367bad3b9f3af8`; the committed golden is untouched.
Fresh differential and saved-wire replay **82/82 passed in both KYC states**,
including hidden-value privacy and both SQL cancellation proofs. All runs ended.
Logs: `work/rust-phase1/task11-m2-{record,on,off}-stable.log`.
An extra combined Python run without explicit test database settings failed one
lifespan test on refused local port 5432 (219 passed). Re-running with explicit
in-memory SQLite passed **220 tests** (oracle plus contract suites); no live host
was contacted. The required standalone contract suite passed **119 tests**.

Commands for the next complete checkpoint (run from this worktree):

```bash
export RELEASE_SHA=$(git rev-parse HEAD)
cargo fmt --manifest-path rs/Cargo.toml --all -- --check
cargo clippy --manifest-path rs/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path rs/Cargo.toml --workspace
RUST_TEST_PYTHON=$PWD/venv/bin/python bash rs/read/tests/run_postgres.sh
venv/bin/python -m pytest tests/rust_contract -q
# Record the expanded corpus twice in each state, then replay that exact source.
venv/bin/python -m scripts.rust_contract.record_mcp
export MCP_LANE_EVIDENCE=$PWD/work/rust-phase1/mcp-evidence/golden/$RELEASE_SHA
venv/bin/python -m scripts.rust_contract.run_disposable --group mcp --seed-mode corpus --kyc-gate enabled
venv/bin/python -m scripts.rust_contract.run_disposable --group mcp --seed-mode corpus --kyc-gate disabled
```

## Latest repair status and PM decision inputs — 2026-10-16

### Shared port authority and modern negotiation (supersedes blockers below)

`5ed6326b` moves kernel range discovery into `tests/ephemeral_ports.py` and uses
it from the allocator, Python producer and target consumer. All fixed-range
producer/consumer guards found in the repository now share that helper. Discovery
fails closed; no cache or host configuration change is used. Numeric loopback,
token, environment, release identity, database `_test` authority and all other
checks are unchanged. **Shared-harness merge:** the build owner and Phase 0
owner must reconcile this helper and imports at merge. Twelve port tests include
Linux/macOS parsing, read failures, valid Linux port 38153, and invalid ports.
The existing target-attestation suite still passes. One initial test invocation
used nonexistent `tests/unit/` paths and ran no tests; the corrected actual suite
`tests/test_disposable_target.py` passed with the contracts (167 at that point).

The shared rule removed both Linux blockers. At fixed source `5ed6326b`, remote
recording was stable twice in each state and fresh differential plus saved-wire
replay passed **132/132 in both KYC states**, including privacy and both SQL
cancellation proofs. Evidence: `work/rust-phase1/task11-shared-port-wire.log`.

`4e2f81da` adds four modern negotiation cases to the HTTP oracle matrix:
modern/future initialize both retain Python's `2025-11-25` fallback; modern
validation errors include `resultType` and server identity; modern tools/list
uses private cache scope, Python field order and modern schema order. Failing
oracle tests preceded the response fixes. Native rmcp still owns dispatch.
The corpus now has **136 cases**. HTTP fixture regeneration is tested directly
against the local Python ASGI oracle without SQL. New full remote gates run only
through `verdaxis-prod-test`; the single committed golden is unchanged.
Local final checks: fmt/clippy pass; Rust workspace **143 passed**; Python
contracts **134 passed**; contracts plus disposable-target checks **168 passed**;
combined Python oracle/contracts/target checks **269 passed**. The final isolated
Linux PostgreSQL suite passed **152 tests**, including typed failure recovery.
Logs: `work/rust-phase1/task11-modern-final-{offline,python,oracle,remote}.log`.
The final remote source is `4e2f81da0a719b508dbc556dad7d91a0f6c4c0a7`.
Both KYC states recorded **136 stable cases across two passes**; fresh differential
and saved-wire replay each passed **136/136 in both states**, with recursive
hidden-value privacy and both SQL cancellation proofs. All runs finished and the
wrapper exited zero. No source nondeterminism or comparator change occurred.
Remote lane evidence remains under that source's untracked
`work/rust-phase1/mcp-evidence/golden/` directory; logs are streamed locally.

**Current resume / integration handoff:** shared range and the requested modern
initialize/tool-list/typed-error negotiation cases are complete for this corpus.
PM reconciles shared harness changes with the build and Phase 0 branches, then
publishes MCP into the single merged golden. Do not treat the historical STOP
sections below as current blockers. Broader unrecorded protocol states (for
example GET wildcard/resumption and modern notification combinations) are not
claimed as exhaustively accepted by this 136-case gate. An independent acceptance
review can now use these exact commits and evidence without another environment
repair. No test process remains running.


### PM-approved allocator/root-argument checkpoint (supersedes stops below)

- `bf8f7776`: **build-owner merge required**. Minimal PM-approved change to
  `scripts/rust_contract/run_disposable.py`: Linux procfs/macOS sysctl range
  discovery, fail-closed parsing, then kernel-assigned port containment. Eleven
  failing-first tests cover both platforms, malformed/unreadable ranges and port
  38153. No host setting changes.
- `145afb1b`: root arguments list/number/string/bool now return exact HTTP-200
  `-32602` method errors; null becomes `{}` before existing typed validation.
  Five exact wire regressions failed first, then passed.
- `e11931fc`: seven failing-first Python HTTP vectors cover Origin first,
  Content-Type before Host, Accept before JSON parsing, exact inner content-type,
  JSON-only/wildcard Accept and modern catalog success. rmcp natively supports
  modern dispatch; do not rewrite its version header. The adapter appends Python's
  modern serverInfo metadata after result serialization. This proves modern
  catalog success, not every initialize/notification/validation-error variant.
- Local fmt/clippy pass; workspace **143 passed**, Python contracts **132 passed**.
  The corpus now contains **132 MCP cases**. The single golden is untouched.

**New STOP / exact resume:** remote recording through `verdaxis-prod-test` now
passes kernel range discovery but Python server startup refuses port **46487**:
`tests.disposable_server.ServerConfigError: disposable server requires numeric
loopback ephemeral bind`. `tests/disposable_server.py:39` and
`tests/disposable_target.py:19,63` still hard-code the 49152 minimum. These are
integration safety guards, outside the PM's specifically approved allocator
change. No host setting or guard was changed. The run ended with cleanup; no
new recording/differential/replay completed. Evidence:
`work/rust-phase1/task11-pm-gates.log` (remote source `145afb1b`). Last full wire
pass is still **105/105**. No source nondeterminism was found.

PM must decide how to align producer and consumer ephemeral-range guards while
retaining numeric-loopback, token, environment and SHA attestation. After that,
record the 132 cases and run both KYC differential/replay gates through the
wrapper at a fixed HEAD. Continue modern initialize negotiation and modern tool
validation-error metadata with failing oracle vectors, then GET wildcard/header
precedence; do not claim broad transport acceptance from the seven new vectors.
All launched runs finished. Colima remained off; no direct SSH was used.


**STOP / resume:** the authorized Linux runner passed **150 PostgreSQL tests**
(including typed-error recovery), but MCP recording stopped before server startup:
`RunnerError: kernel returned a non-disposable port 38153` at
`scripts/rust_contract/run_disposable.py:66`. The helper assumes OS ephemeral
ports are 49152–65535; this Linux kernel assigned 38153. Per the founder's rule,
do not change host setup or relax target attestation. PM must authorize a
repository-side bounded high-port allocator or provide a compatible test runtime.
No new 120-case recording, fresh differential or replay completed. The last
complete wire checkpoint remains 105/105. No nondeterminism was observed.
All launched tests ended; Colima was not started and no direct SSH was used.

Local checks: fmt/clippy pass; workspace **141 passed**; Python contracts
**121 passed**; combined Python oracle/contracts **222 passed**. Remote final
source was `a56b669fbccce786e1d4791575e5e376c7e1c284`. Evidence:
`work/rust-phase1/task11-remote-final.log`, `task11-final-offline.log`,
`task11-final-python.log`, and `task11-final-oracle.log`. The earlier remote run
also passed, but predates the additional typed admission test; use the final
150-test result. The single committed golden remains untouched.

After the environment decision, run recording and both KYC differential/replay
passes through `verdaxis-prod-test` only, at a fixed HEAD. Then resolve the
supported-modern success branch and root-argument PM decisions below. Do not
claim full Task 11 acceptance from the offline protocol rejection matrix.

The founder moved PostgreSQL/Docker/Linux tests to
`verdaxis-prod-test "$PWD" 'COMMAND'`. Colima remains off. Never run the old
local Docker commands from earlier checkpoints. No direct SSH or host setup
change is authorized.

M3: JSON-RPC envelope validation now precedes tool argument adaptation. Eleven
Python SDK oracle rejection cases cover wrong/missing version, array envelope,
wrong method/params, absent/empty/partial modern metadata, header mismatch and
unsupported modern version. rmcp still dispatches accepted requests. Full modern
success negotiation and all HTTP header precedence are not newly proven by this
rejection matrix; do not claim full protocol-domain acceptance from it.
A local Python probe confirms `MCP-Protocol-Version:2026-07-28`,
`Mcp-Method:tools/list`, and `_meta` containing that version plus empty client
capabilities returns HTTP 200 tools. rmcp's accepted-version dispatch still needs
an explicit compatibility test/adapter for this success branch; M3 is therefore
repaired for reviewed rejections, not closed for every modern request.

N1: `ApiErrorKind::{Expected,Database,Unexpected}` is internal-only classification.
The broad constructor edits add this field without changing REST status/detail.
Pool acquire/cleanup, SQL reads and shared database classifiers mark Database;
projection failures and task joins mark Unexpected. Both tools use the same
mapping. The failing-first message test, closed-pool tests and a disposable
admitted-work test distinguish internal failure from real SQL division-by-zero
and check recovery. A worker-panic cleanup proof is not added by this checkpoint.

M2 PM decision: root arguments require matching SDK method-parameter validation,
not more Pydantic request-field patches. Exact Python wire probes (no database)
are in `work/rust-phase1/task11-pm-root-arguments.txt`:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"plan_voyage_fuel","arguments":[]}}
```

Python HTTP 200, exact body (also for `arguments:42`):

```json
{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Invalid request parameters","data":""}}
```

For the same request with `arguments:null`, Python HTTP 200, exact body:

```json
{"jsonrpc":"2.0","id":1,"result":{"content":[{"text":"Error executing tool plan_voyage_fuel: 1 validation error for plan_voyage_fuelArguments\\nrequest\\n  Field required [type=missing, input_value={}, input_type=dict]\\n    For further information visit https://errors.pydantic.dev/2.13/v/missing","type":"text"}],"isError":true}}
```

Rust currently validates these as root tool models instead. This remains an
unapproved difference for PM disposition, not source nondeterminism. Do not
expand into a second general SDK implementation without an explicit decision.

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
