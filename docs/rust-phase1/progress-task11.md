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
