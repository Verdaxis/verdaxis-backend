# Same-meaning Tasks 9–10 closeout — 22 October 2026

Scope: “Resolve curve, reference-export, and MCP slice costs” and “Resolve orderbook aggregation and kept-page costs”. The PM descoped new SQL rewrites, so this release adds no SQL rewrite. No deployment was attempted. Latency, memory and load measurement: **not run (decision 110)**.

## Changes retained

- `ea97739e`: listings pages compared with the full eligible ranking, including zero limit and offsets at and past the end, with KYC on and off.
- `83b5470e`: MCP old-shape 13-bind regression check and file-based plan-JSON transfer in the plan harness.
- `9caf7721`: the curve, board and MCP statements use the shared relation again (as before `be61502a`). The keyed candidate shape stays only in test support, for the equivalence tests and plan diagnostics.
- `cd3663ef`: formatting only.
- `aa590f09`: the workspace PostgreSQL run skips the curve and orderbook equivalence tests by test name. They run in `run_market_contract.sh` against the seeded fixture database.
- The uncommitted candidates (formal-trade projection marker, reference and curve helpers, orderbook aggregate projection) were not kept.

## Shared curve-key decision

Disposable large database, parent versus `be61502a`; shared hit + read blocks, no timing:

| Statement | Parent | Candidate |
| --- | ---: | ---: |
| R09 table orders | 65,234 | 65,234 |
| R10 slice orders | 65,230 | 65,230 |
| R11 board orders | 65,234 | 65,234 |
| MCP resolve prices | 65,230 | 65,230 |
| MCP market slice | 65,224 | 65,224 |

Both plan calls exited 0. R11 temp read/write was 534/535 for both shapes, and the returned row counts matched. No statement reduced plan work, so `9caf7721` reverts the runtime change.

## Gates

Staging test host, through `heavy run`:

| Gate | Commit | Result |
| --- | --- | --- |
| `cargo fmt --check` | `cd3663ef` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | `cd3663ef` | exit 0 |
| Offline workspace tests (`--locked --offline`) | `cd3663ef` | exit 0 |
| `run_market_contract.sh`: market PostgreSQL 6, curve equivalence 7, orderbook equivalence 7 | `cd3663ef` | exit 0 |
| `tests/rust_contract/test_explain_routes.py` | `cd3663ef` | 2 passed, exit 0 |
| `run_postgres.sh` (workspace PostgreSQL suite) | `aa590f09` | 253 passed, 20 skipped by name, exit 0 |
| Large meaning simulation, `phase1-meaning-v1` | `cd3663ef` | 431 pairs, 0 differences; 10 of 11 groups match |

The `prices` group of the meaning simulation was inconclusive twice (the full run and a `prices`-only run): its two Python recordings did not agree, so the simulation did not compare Rust for that group, and its gate field reads no-go. `aa590f09` and this record change no Rust or SQL source after `cd3663ef`.
