# Task 14a — the go/no-go gate, the harnesses, and the measured comparison

Date: 2026-10-17. Branch `feat/rust-phase-1-20261015`, worktree base `c758739d`.
No live access, push, agents, subagents or skills. Every build and test ran
through `verdaxis-prod-test "$PWD" '…'`. Total test-host run time stayed inside
the four-hour budget.

## Scope (founder decision 48, 16 Oct)

No seven-day shadow clock. Task 14 splits:

- **14a (this task):** the gate, the harnesses, the workload manifest, the
  seeded differential simulation, the memory/CPU/latency measurement, and the
  go/no-go report draft — everything that runs on the test host.
- **14b (later):** one short read-only differential pass on staging data after
  the operator deploys Rust. The `shadow` harness and its tests are built here
  with `scripts/rust_contract/staging.example.toml`; no command against staging
  or production was run.

## What was built

| File | Purpose |
| --- | --- |
| `rs/contract/src/gate.rs` | `passes_gate`: zero unapproved differences, Rust memory ≤ half of Python, Rust p95 ≤ Python p95 |
| `rs/contract/tests/gate.rs` | false-pass tests: NaN/infinite/zero-time p95, zero baseline, one difference, +1 byte, +0.1 s |
| `tests/rust_contract/workload.json` | the committed workload manifest (sizes, seed, weights, queries, size bands) |
| `scripts/rust_contract/workload.py` | manifest load/validate, weighted deterministic schedule, observation math |
| `scripts/rust_contract/workload_seed.py` | seeded bulk workload at a declared size, through the attested fixture writer |
| `scripts/rust_contract/simulate.py` | seeded differential simulation with the Python–Rust–Python sandwich |
| `scripts/rust_contract/shadow.py` | bounded read-only shadow harness (run only in 14b) |
| `scripts/rust_contract/measure.py` | memory/CPU/latency measurement over the identical workload |
| `scripts/rust_contract/report.py` | measured-only report assembly and Markdown render |
| `scripts/rust_contract/staging.example.toml` | the reviewed `--config` template, no secrets |
| `tests/rust_contract/test_shadow.py`, `test_measure.py` | host-free tests for every safety and arithmetic rule |

## Workload manifest

Committed **before** any measured run (`3725e9ee`). Sizes: **1,000 / 100,000
orders**, **1,000 / 100,000 historical trades**, **100 / 10,000 news rows**.
Distribution seed `20261017`. 27 public data routes with an equal-weight
baseline and a declared representative mix; three MCP tools. Every route
carries its measured query and the response-size band taken from the recorded
corpus goldens. Bulk orders use the scalar fuels only; the typed FAME and LNG
products keep their corpus fixtures. No LNG demo or benchmark price is seeded.

Manifest digest:
`3f7653e132ed925750eef6b12c8d449205a0c904dced58c800eccc9d6d6e5e14`.

## Seeded differential simulation

Both sizes, every group, Python–Rust–Python sandwich. Two Python recordings
agreed at every case (no `data_changed` classification); 487 pairs per size.

| Size | Groups matched | Differences |
| --- | --- | --- |
| small | 10 of 11 | 9, all `/api/trape`-class trade-tape item ordering |
| large | 10 of 11 | 33, same class |

Every group except `prices` matched byte for byte at both sizes, including
`mcp` (143 cases) and `curves_canonical` (137 cases). All differences are
`/api/trade-tape` item order inside `confirmed_at` tie groups. A diagnostic
proved the item **set** is identical and Python is self-consistent; the order
within the tie group follows the physical plan. `/api/trade-tape` has no
tie-breaker in either implementation (`app/routers/trade_tape.py:129`;
`rs/read/sql/prices/tape.sql`) and no reviewed tie exception (news has
decision 18, price summaries decision 22, orderbook lists the decision-23
tie-breaker). This is source-level tie nondeterminism, not a Rust port defect;
the fix needs a PM decision (tie-breaker in both, or a tie exception) plus a
golden re-record. No route or economic code was changed in this task.

## Measurement

Two runs per size (Python cell then Rust cell, one repetition, alternating
order across repetitions not needed with one repetition per size). Warm-up
15 s; windows 15 s @ 1 RPS + 900 s @ 5 RPS; equal-weight baseline;
end-to-end through Caddy; four-worker Python in one systemd user unit versus
one Rust release process in another; cgroup v2 counters sampled once per
second. The load generator and the disposable PostgreSQL container ran outside
both service cgroups.

| Size | Python median | Rust median | Ratio | Python CPU | Rust CPU | Mix p95 py→rs |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| small | 802.3 MiB | 43.0 MiB | 5.4% | 0.773 cores | 0.118 cores | 0.984 → 0.499 s |
| large | 1059.6 MiB | 206.3 MiB | 19.5% | 1.301 cores | 0.090 cores | 11.576 → 10.721 s (capacity-bound) |

Small-size per-route: 23 of 30 measured units have lower Rust p95; four
regressions — R20 (+0.041 s), R24 (+0.022 s), `plan_voyage_fuel` (+0.004 s),
and R27 (`/api/listings`, bimodal Rust tail, p95 0.946 vs 0.167 s). Large-size
per-route p95 is capacity-bound (2,580 Python / 3,211 Rust busy responses at
the plan's 5 RPS ceiling) and is not a workload-shaped comparison; memory and
CPU remain comparable.

Coverage: every route received about 150 (small) / 50 (large) completed
observations, below the manifest's 1,000 floor — reported inconclusive with the
numbers needed (~7x the window at 5 RPS for the small size).

## Gate outcome

- **Zero unapproved differences: no-go** — the trade-tape tie-order class.
- **Memory ≤ 50%: pass** at both sizes (5.4% and 19.5%).
- **p95: pass on the mix at the small size; four per-route exceptions**, and
  the plan's "every route's p95" bar is not fully met.

Overall: **NO-GO as measured**, solely on the trade-tape difference class.
Details and raw-evidence digests: `docs/rust-phase1/go-no-go.md`.

## Harness defects found and fixed by the runs themselves

1. The measurement's explicit portless `Host` header made the MCP transport
   refuse every call (421); it now sends the corpus's own header set including
   the exact `Accept: application/json, text/event-stream` (406 without it).
2. R02's measured query was the port-not-found case with no found-port golden;
   it now measures the seeded `T4B_ONE` port.
3. The first small run timed out at 10 RPS; the offered load is now capped at
   the plan's 1 and 5 RPS, and the first (saturated, 10 RPS) large run is kept
   only as capacity evidence.
4. The workload seeder's `:param::type` casts broke SQLAlchemy's bind parsing,
   asyncpg wanted real lists for array parameters, the order insert needed
   `created_at`/`updated_at`, and DEMO OPEN rows need a future expiry
   (`ck_orderbook_orders_domain`).

## Verification (test host, each command's own exit code)

| Gate | Result |
| --- | --- |
| `SQLX_OFFLINE=true cargo test --workspace` | 0 (all targets) |
| `cargo fmt --all -- --check` | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `SQLX_OFFLINE=true cargo test -p verdaxis-contract --test gate` | 4 passed |
| `venv/bin/python -m pytest tests/rust_contract/test_shadow.py tests/rust_contract/test_measure.py -q` | 60 passed |
| `ENVIRONMENT=test … pytest tests/unit -q` | 2,411 passed |
| `venv/bin/python -m pytest tests/rust_contract -q` | 508 passed |
| Simulation small / large | exit 0 both |
| Measurement small / large (v2 configs) | exit 0 both |

## Commits

`195f412a` gate; `3725e9ee` manifest; `7c1e3a08` shadow; `825c0f86`
simulation+seeder; `e6839d62` seeder fixes; `518adc6a` seed-target env;
`7a1e7a58` measurement+report; `8bc9de71` report CLI+configs; `251d50c6`
measurement fixes; `e43282ae` MCP Accept + load fit; plus this report and
progress commit.

## Resume point

Task 14a is complete. Next: the PM decides the trade-tape tie question
(tie-breaker in both implementations or a tie exception) and the golden
re-record; then Task 14b's staging differential pass after the operator
deploys Rust to staging. Nothing in this task authorized deployment.
