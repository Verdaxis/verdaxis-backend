# Same-meaning Tasks 9–10 closeout — 22 October 2026

Scope: “Resolve curve, reference-export, and MCP slice costs” and “Resolve orderbook aggregation and kept-page costs”. The PM descoped new SQL rewrites. No deployment was attempted. Latency, memory and load measurement: **not run (decision 110)**.

## Changes retained

- `ea97739e`: listings pages compared with the full eligible ranking, including zero limit and offsets at/past the end, with KYC on/off.
- `83b5470e`: MCP old-shape 13-bind regression check and file-based plan-JSON transfer. Both harness tests passed on the staging test host (exit 0).
- Rejected runtime changes were restored file by file: the formal-trade projection marker, reference/curve experiment helpers in `visibility.rs`, formatting in `curve_unknown_metadata.rs`, and the aggregate projection call in `orderbook_map.rs`. Scratch and candidate patches remain in ignored `work/`; root `work_ref*.rs` files were moved there.

## Shared curve-key decision

Disposable large database, parent versus `be61502a`; shared hit + read blocks, no timing:

| Statement | Parent | Candidate |
| --- | ---: | ---: |
| R09 table orders | 65,234 | 65,234 |
| R10 slice orders | 65,230 | 65,230 |
| R11 board orders | 65,234 | 65,234 |
| MCP resolve prices | 65,230 | 65,230 |
| MCP market slice | 65,224 | 65,224 |

Both plan calls exited 0. R11 temp read/write was 534/535 for both shapes. Returned row counts also matched. No statement reduced plan work. Restore the parent runtime constructors; retain the rejected keyed shape only in test support for equivalence and diagnostics. Existing test registrations and bind lifetimes remain.

## Gate checkpoint and resume

**Blocked; not ready to push.** The final gate wrapper exited **255 before starting a remote command**: SSH to staging `194.233.68.86:22` refused all six attempts. A separate read-only SSH check also refused. No lane test run remains pending from that call. Final-source fmt, clippy, offline tests, curve/orderbook equivalence, market PostgreSQL and large meaning simulation have **not run** on the restored source. No push was made.

When the staging test host is available:

1. Run host formatting and bring its exact patch back; inspect it before applying. Run fmt check, clippy `--workspace --all-targets -- -D warnings`, and locked workspace offline tests through `heavy run 3600`.
2. Run `bash rs/read/tests/run_market_contract.sh` (includes market PostgreSQL and both equivalence suites), plus `venv/bin/python -m pytest tests/rust_contract/test_explain_routes.py -q`.
3. Run `venv/bin/python -m scripts.rust_contract.simulate --size large --profile phase1-meaning-v1 --rules-sha256 f68125dd74d59f0af49f120b839d8e3cdadef1deca8027f9d0f08fc8a0b99e0e` through the staging wrapper and `heavy run`. Wait for every `CALL_EXIT=` marker.
4. Fix any failures, commit small logical changes, update this gate record, and push only `HEAD:refs/heads/feat/rust-same-meaning-t9-10` when green. Read CI under the PM rules.

Local evidence: `work/closeout-{plans,mcp-exact,gates}.log`. The gate log records the connection failure, not test results.
