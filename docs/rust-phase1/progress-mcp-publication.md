# Unified MCP and REST golden publication — 2026-10-16

## Authority and source

The founder moved every PostgreSQL/Docker/Linux test to `verdaxis-prod-test "$PWD" 'COMMAND'`. Colima and local Docker remained off. No direct SSH, production service operation or test-host setup change was performed. The approved wrapper owns transport, isolated checkout and the capped test user.

Oracle source: `78af98c499e1e05268418f87384cb15b58af9df9` (merged Task 11). Publication commit: `bef75fca`.

## Recordings

Task 11's `McpRecorder` recorded 143 cases twice with KYC enabled and twice disabled. Both were stable. It wrote its normal remote lane evidence, including schema metadata. The shared recorder separately recorded all other groups twice in both states with zero unstable paths and zero failures.

One directory now holds 758 case directories: 379 per state. Per state: simple_reads 29, prices 40, curves_legacy 4, curves_canonical 125, orderbook_snapshot 3, orderbook_lists 12, orderbook_map 9, echo 2, health 8, live_benchmarks 4, mcp 143.

The wrapper does not retrieve generated files automatically. The recorder outputs were returned as a base64 tar archive through an authorized wrapper command's stdout. Local extraction accepted only regular files below the exact SHA, checked all 758 metadata source identities, and preserved the original recorded bytes. No golden file was hand-edited. The prior 9880b714 directory is retained only in ignored `work/rust-phase1/mcp-publication-old-golden`.

Policy, MCP catalog and manifest, catalog/terms vectors, public schema field-order data, curve policy and voyage/tool artifacts were regenerated from this source. Non-provenance values remain source-derived; unchanged vectors retain their bytes.

## PM correction and resumed verification

The earlier stall diagnosis below was wrong: Cargo buffered the test output. The PM confirmed the remote loop continued past legacy curves and reached MCP. The PM stopped that loop and the queued database call; no lane container remains and the lane lock was clear before this resumed run. The wrapper now cancels its remote run on interruption, but resumed calls are not interrupted. Silence is not treated as a stall.

Founder decision 52 moves **all builds and tests**, including fmt, clippy, offline Cargo and pytest, through the approved wrapper. No Cargo, pytest, Docker or Colima runs locally in this continuation. Each host call runs alone, writes a local `gate-*.log`, and ends with `CALL_EXIT=`. Golden publication and oracle remain unchanged.

## Completed sequential host gates

Source checked: `ddfa79706e3961e8847d910326a3b8bade4ca258` plus this documentation-only update. All calls used the authorized wrapper, ran one at a time, and ended with a recorded `CALL_EXIT`. None was interrupted. No local build/test ran and no golden/artifact changed.

| Group | Corpus result | Extended target / call result |
| --- | --- | --- |
| curves_legacy | 4/4 matched | 60 matrix + 8 scale comparisons passed; exit 0 |
| curves_canonical | 125/125 matched | 112 probes and measured checks passed; exit 0 |
| orderbook_snapshot | 3/3 matched | Failed at `orderbook_snapshot_oracle.py:314`, exact `info["Config"]["Image"] == IMAGE` assertion; exit 1 |
| orderbook_lists | 12/12 matched | 24 integer boundary comparisons and 15 CI overlays passed; failed at `orderbook_lists_oracle.py:59` in `check_query_count`, same image assertion; exit 1 |
| orderbook_map | 9/9 matched | 97 edge comparisons passed; exit 0 |
| echo | 2/2 matched | exit 0 |
| health | 4 known difference paths | H04 HEAD headers/status; H05 POST and H06 OPTIONS Allow values; exit 1, Task 13 |
| mcp | 143/143 matched | All three tools' recursive privacy and both SQL cancellation proofs passed; exit 0 |
| live_benchmarks (ordinary) | 4/4 matched | 3 base + 7 acceptance cases passed; exit 0 |

Snapshot/list failures are test-harness image-name equality assertions on the Podman-backed runtime, not observed response differences. The actual inspected image string was not printed; no claim is made that the pinned digest differs or that the name mismatch is safe to ignore. No comparison or image guard was weakened, and host setup was unchanged. Follow up with a reviewed container-image identity attestation compatible with the authorized runtime before claiming these extended targets pass.

**Legacy timing:** host command `real 1m33.548s`, `user 0m38.004s`, `sys 0m16.194s`; Rust `legacy_curve_contracts_match` completed in **17.67s**. It did not exceed ten minutes, so no slow-function investigation was required. The earlier silence was buffered output, not a stall.

**Suites:** PostgreSQL runner **152 passed**, two dedicated market tests explicitly filtered; separate market runner **2 passed**, selecting 14/46 orders and 4/6 trades. Host fmt/clippy passed; offline workspace **143 passed**. Host Python contracts **134 passed**, five warnings. Every suite call exited 0. No full Python unit suite was requested or rerun in this continuation.

Evidence: local `work/rust-phase1/gate-{curves_legacy,curves_canonical,orderbook_snapshot,orderbook_lists,orderbook_map,echo,health,mcp,live_benchmarks,postgres,market,offline,python}.log`. All thirteen calls reached `CALL_EXIT`; no test or waiter remains running. Simple reads/prices retain the prior completed evidence and were not in this continuation's requested sequence. Resume with the two image-attestation harness failures and Task 13 health routing; the published golden and oracle remain unchanged.

## Previous verification and early stop (superseded diagnosis)

- Local fmt/clippy passed; offline Rust workspace **143 passed**; Python contract suite **134 passed** after publication.
- Authorized Linux simple_reads **29/29 passed** including its target; prices **40/40 passed** plus both populated-probe passes.
- Legacy curves **4/4 corpus cases matched**, but the extended target stalled after `legacy_curve_contracts_match has been running for over 60 seconds`. Last output was around 01:57 UTC; still no output at 02:14 UTC. No terminal target result was received. Do not claim this target passed.
- The local wrapper invocation, queued database-suite invocation and output-only waiters were stopped. This does **not** prove remote systemd process/container cleanup: the wrapper has no cancellation/report-back API, and no direct SSH or host setup changes were permitted. PM must confirm the isolated test job and lane lock are clear before resuming.
- Not completed: remaining corpus groups (canonical curves, snapshot, lists, map, echo, health, MCP), ordinary benchmark target, PostgreSQL runner and market matrix. The database suites were queued behind the stalled lane lock and produced no test results. Known health differences were not remeasured in this run.

**Exact resume:** preserve oracle `78af98c499e1e05268418f87384cb15b58af9df9` and published bytes. After PM confirms isolated test-job cleanup, diagnose the extended legacy probe through the approved wrapper with bounded per-group execution; do not start local Docker or loosen comparison. Then run the remaining groups and both DB suites one command at a time via module invocation. No rerecord is needed unless Python/fixture/case sources change. This is an incomplete verification checkpoint, not full acceptance.

## Invocation issue and resume rule

Direct `venv/bin/python scripts/rust_contract/run_disposable.py` failed on Linux before recording with `ModuleNotFoundError: tests` when importing the shared ephemeral-range helper. This is script-path import behavior, not a missing host dependency. `venv/bin/python -m scripts.rust_contract.run_disposable` uses the repository root and succeeds without changing host setup or relaxing guards. Use module invocation through the wrapper for every group. The two failed direct-script attempts published nothing; their containers were cleaned by the runner. An output-only waiter was stopped after those runs ended.

MCP evidence is remote under `work/rust-phase1/mcp-evidence/golden/78af98c499e1e05268418f87384cb15b58af9df9`. Local logs: `mcp-publication-record.log`, `mcp-publication-transfer-module.log`, `mcp-publication-groups.log`, `mcp-publication-db-suites.log`, and `mcp-publication-final-{offline,python}.log` under `work/rust-phase1/`.
