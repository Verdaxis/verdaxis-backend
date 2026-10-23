# Trading route cache integration plan

> Execute in this worktree without subagents. The PM brief and same-meaning plan Task 8 / section 7.6 govern this work.

Goal: default-off public cache wrappers on the accepted release-2 route implementations, with no deployment or activation.

Architecture: retain the accepted runtime helper and hit proof. A shared route adapter owns keys, cache registration, the captured clock and same-snapshot temporal facts. With caching disabled it calls the existing admitted loader without extra SQL. Existing route loaders retain their SQL and transaction-local settings. Successful public JSON alone is retained; response headers and generated timestamps are rebuilt per request. Coarse global minima may cause extra misses but cannot omit a relevant row. Unknown facts bypass storage.

## Merge ledger

- `509c13b0`: no-ff merge of accepted Task 9–10 `c599dddb`; no conflicts. Reviewed diagnostic visibility accessors, curve statement accessor, test registrations and exact named PostgreSQL skips. No route SQL rewrite retained.
- `c40de2dc`: no-ff merge of staging `3bd89ae9`; no conflicts. Reviewed legacy JIT transaction and shared tape normalization/harness fixes. No Cargo lock conflict or manual resolution.

## Checks and execution

1. Commit route-level regressions before wrappers. Exercise actual HTTP handlers for snapshot, lists, map, prices and canonical curves with cache enabled and disabled. Use deterministic observer clocks and checkpoints, actual committed fixture changes, no listener event consumption. Assert cache population and reuse as well as payload meaning. Include authority rotation/reused generation and time equality where applicable; existing runtime schedules continue to test two processes and incarnations.
2. Implement one shared route adapter and register it. Add per-AppState public cache, typed route+query identity, and exact single clock capture. No auth/user/header values in keys.
3. Derive conservative temporal bounds in the outer read-only repeatable snapshot: every future order expiry and KYC grace cutoff, Singapore/UTC midnight where relevant, rolling trade departures (including quarantined trades), signal window departures, and observed-at freshness changes. At an inclusive moving-window edge, bypass storage. Rebuild nested generated_at values rather than retaining request time. Any unproved surface stays uncached and is recorded.
4. Add wrappers only to the five assigned modules. Keep private/write/listings/tape/MCP ownership unchanged. Validate before lookup. Preserve normal headers, CORS, rate limits and admission on hits.
5. Commit wrappers, then remote formatting if needed. Never run builds/tests on this Mac. One final source-bound batch per host: remote M1 Rust gates and audit; one staging batch RED on tests-only SHA then GREEN cache-event, market, Rust PG, Python unit/monitor/rust_contract, one phase1 meaning gate and large prices simulation. Use 7200 seconds for meaning. Keep the same queued call; no duplicate. No measurements.
6. Update architecture/evidence/private progress with exact source SHA, UTC, commands, exits and conflicts. Push only feat/rust-same-meaning-t8. Watch draft PR 114 CI to completion; do not edit PR status or merge it.

Resume point: merges completed; route regressions and wrapper implementation pending. Preserved untracked .claude/. No remote gate is running for this integration batch.
