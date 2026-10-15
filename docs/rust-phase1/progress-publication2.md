# Integrated publication after Tasks 7b, 7c and 10b

## Oracle and publication

- Base merge: `f2dd2a44`; read all three lane handoffs before work.
- `9880b714` records decision 23 in the plan. R18/R26 use `created_at DESC, id DESC`; this is the approved Python pagination bug fix, not a comparison exception.
- Clean oracle source: `9880b7144a38d188b0ba8583d7479a6c5dd6eb67`.
- `19e29781` publishes the complete integrated corpus and regenerated artifacts. Both fixed-clock recordings completed with `TOTAL_UNSTABLE=0; RECORD_FAILURES=0`, KYC explicitly enabled/disabled. MCP is excluded under decision 19.
- 236 cases per state, 472 total: live_benchmarks 4; health 8; echo 2; simple_reads 29; snapshot 3; lists 12; map 9; prices 40; legacy curves 4; canonical curves 125.
- Old `a417cf51` golden moved to ignored `work/rust-phase1/publication2-old-golden`. No golden bytes were hand-edited. Policy, MCP catalog, manifest, catalog vectors and all nine terms vectors were regenerated. Terms bytes did not change.
- Source-owned `orderbook_query.json` schema field order and enums were regenerated from Python models. `curve_policy.json` enums, seed quotes, defaults and B100 disclaimer were regenerated from Python source, not relabelled blindly; its prior SHA was still `a127672`. Both now bind to the accepted oracle.
- Recording logs: `work/rust-phase1/publication2-{enabled,disabled}.txt`. The recording includes the 10b bulk/depth/subsecond/signal fixtures across all groups, not only curves.

## Initial full Python unit gate

2027 passed, 19 failed, 504 warnings. Every failure is in `tests/unit/test_deploy_health.py` and is the known macOS Bash 3.2 `mapfile` limitation. No other Python unit failure. Log: `work/rust-phase1/publication2-unit.txt`.

## Integration repairs and failure evidence

- The merged map test-server dispatch did not mount listings after decision 23 moved R27/negative cases into `orderbook_map`. Initial run: eight difference paths, both compatibility cases 404. Added the existing listings router to that test group; all nine corpus cases then passed.
- The map edge probe separately searched `orderbook_lists` for those moved cases, producing `ContractError: no cases to record`. It now reads `orderbook_map`; no case is removed or comparison relaxed.
- List corpus initially timed out on Rust R26's two-row page (Python ~1.8 seconds). Ordinary live-benchmark corpus cases passed but its broader matrix timed out. The shared eligibility subplans and new bulk corpus trigger costly PostgreSQL JIT. Scoped benchmark JIT-off made the ordinary matrix pass (four cases plus ten probes, 170 seconds total), but did not fix list pages. List transaction-local JIT-off is tested separately. All filters, limits, clocks and exact comparisons stay unchanged. Benchmark restores the caller's previous JIT value after a successful statement; rollback restores it after failure. List transaction scope restores pool state at commit/rollback.
- Initial and intermediate failures remain in `publication2-orderbook_lists.txt`, `publication2-live_benchmarks.txt`, `publication2-orderbook_map.txt`, `publication2-lists-recheck.txt`, and `publication2-map-recheck.txt`. They are not counted as passing gates.

- After planner repair, lists matched all 12 corpus cases but the lane's exhaustive probe failed at offset 100: it assumed `limit=100` was an uncapped full baseline even when `total` exceeded 10,000. Exhaustive two-row traversal is now scoped to the existing bounded SPOT fixture with an explicit <=100 assertion; additional unfiltered comparisons cover offsets 98, 100 and 10000. The full unfiltered corpus remains unchanged. This repairs fixture assumptions, not response comparisons.

- Two further list-probe assumptions were corrected: R18 does not support `availability_window`, so legacy exhaustive traversal uses the supported Bio-LNG product filter; and the non-null CI test selects ASK rows because the expanded first page is now bulk BIDs without CI declarations. All underlying Python/Rust comparisons passed before each fixture assertion failed. Boundary, unfiltered corpus and mixed-sort tests remain.

## Final results

Repairs: `1c559c0d` (dispatch, case ownership, bounded pagination/CI probes); `f69cdf93` (scoped JIT policy). Exact comparison was not loosened. The ordinary benchmark and list failures reproduce before the planner changes and pass afterward; map failures reproduce before dispatch/ownership changes and pass afterward.

| Corpus group | Cases | Final result |
| --- | ---: | --- |
| simple_reads | 29 | matched |
| prices | 40 | matched; both 17-probe passes |
| curves_legacy | 4 | matched; 60 matrix + 8 scale comparisons |
| curves_canonical | 125 | matched; 112 probes and measured row/query limits |
| orderbook_snapshot | 3 | matched; full edge/isolation target passed |
| orderbook_lists | 12 | matched; 136 edges, 24 integer boundaries, 15 CI overlays, mixed-sort and query-count checks |
| orderbook_map | 9 | matched; 97 edge comparisons |
| echo | 2 | matched |
| health | 8 | four known differences: HEAD status/headers and POST/OPTIONS Allow values; Task 13 |

Ordinary live_benchmarks: 4 cases plus three base/seven acceptance probes matched. Health is the only remaining observed Rust difference group. MCP remains deferred to Task 11, not claimed as passing.

fmt/clippy and Rust suites passed after code repair: workspace 125, PostgreSQL 133, market matrix 2 (14/46 orders and 4/6 trades). Python contracts 96 passed. Python units 2027 passed and 19 known deploy-health platform failures. All test runs, including intermediate failed probes and background waiters, finished before final delivery. The final successful list target took 1008 seconds on the expanded corpus; this is functional acceptance, not a claim of production performance.

Logs: `publication2-committed-*` for final source suites; `publication2-lists-accepted.txt`, `publication2-map-final.txt`, `publication2-bench-recheck.txt` for repaired targets; other groups retain `publication2-<group>.txt`. No new oracle recording was needed after Rust/harness-only integration repair: Python source, shared publication fixture and recorded case inventory remained at the accepted oracle.

## Boundaries

Colima, pinned disposable PostGIS image and numeric-loopback ephemeral services only. No push, live host or live database. Every running test is allowed to finish before the final report. Remaining contract questions are reported rather than resolved by widening comparison rules.
