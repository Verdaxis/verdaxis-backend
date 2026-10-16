# Unified MCP and REST golden publication — 2026-10-16

## Authority and source

The founder moved every PostgreSQL/Docker/Linux test to `verdaxis-prod-test "$PWD" 'COMMAND'`. Colima and local Docker remained off. No direct SSH, production service operation or test-host setup change was performed. The approved wrapper owns transport, isolated checkout and the capped test user.

Oracle source: `78af98c499e1e05268418f87384cb15b58af9df9` (merged Task 11). Publication commit: `bef75fca`.

## Recordings

Task 11's `McpRecorder` recorded 143 cases twice with KYC enabled and twice disabled. Both were stable. It wrote its normal remote lane evidence, including schema metadata. The shared recorder separately recorded all other groups twice in both states with zero unstable paths and zero failures.

One directory now holds 758 case directories: 379 per state. Per state: simple_reads 29, prices 40, curves_legacy 4, curves_canonical 125, orderbook_snapshot 3, orderbook_lists 12, orderbook_map 9, echo 2, health 8, live_benchmarks 4, mcp 143.

The wrapper does not retrieve generated files automatically. The recorder outputs were returned as a base64 tar archive through an authorized wrapper command's stdout. Local extraction accepted only regular files below the exact SHA, checked all 758 metadata source identities, and preserved the original recorded bytes. No golden file was hand-edited. The prior 9880b714 directory is retained only in ignored `work/rust-phase1/mcp-publication-old-golden`.

Policy, MCP catalog and manifest, catalog/terms vectors, public schema field-order data, curve policy and voyage/tool artifacts were regenerated from this source. Non-provenance values remain source-derived; unchanged vectors retain their bytes.

## Verification and explicit early stop

- Local fmt/clippy passed; offline Rust workspace **143 passed**; Python contract suite **134 passed** after publication.
- Authorized Linux simple_reads **29/29 passed** including its target; prices **40/40 passed** plus both populated-probe passes.
- Legacy curves **4/4 corpus cases matched**, but the extended target stalled after `legacy_curve_contracts_match has been running for over 60 seconds`. Last output was around 01:57 UTC; still no output at 02:14 UTC. No terminal target result was received. Do not claim this target passed.
- The local wrapper invocation, queued database-suite invocation and output-only waiters were stopped. This does **not** prove remote systemd process/container cleanup: the wrapper has no cancellation/report-back API, and no direct SSH or host setup changes were permitted. PM must confirm the isolated test job and lane lock are clear before resuming.
- Not completed: remaining corpus groups (canonical curves, snapshot, lists, map, echo, health, MCP), ordinary benchmark target, PostgreSQL runner and market matrix. The database suites were queued behind the stalled lane lock and produced no test results. Known health differences were not remeasured in this run.

**Exact resume:** preserve oracle `78af98c499e1e05268418f87384cb15b58af9df9` and published bytes. After PM confirms isolated test-job cleanup, diagnose the extended legacy probe through the approved wrapper with bounded per-group execution; do not start local Docker or loosen comparison. Then run the remaining groups and both DB suites one command at a time via module invocation. No rerecord is needed unless Python/fixture/case sources change. This is an incomplete verification checkpoint, not full acceptance.

## Invocation issue and resume rule

Direct `venv/bin/python scripts/rust_contract/run_disposable.py` failed on Linux before recording with `ModuleNotFoundError: tests` when importing the shared ephemeral-range helper. This is script-path import behavior, not a missing host dependency. `venv/bin/python -m scripts.rust_contract.run_disposable` uses the repository root and succeeds without changing host setup or relaxing guards. Use module invocation through the wrapper for every group. The two failed direct-script attempts published nothing; their containers were cleaned by the runner. An output-only waiter was stopped after those runs ended.

MCP evidence is remote under `work/rust-phase1/mcp-evidence/golden/78af98c499e1e05268418f87384cb15b58af9df9`. Local logs: `mcp-publication-record.log`, `mcp-publication-transfer-module.log`, `mcp-publication-groups.log`, `mcp-publication-db-suites.log`, and `mcp-publication-final-{offline,python}.log` under `work/rust-phase1/`.
