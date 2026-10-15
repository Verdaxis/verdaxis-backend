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
