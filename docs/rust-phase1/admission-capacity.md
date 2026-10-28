# Rust read admission capacity — decision 88

## Approved scope and plan (2026-10-18)

Founder decision 88 removes Python's admission formula from Rust. Rust owns
`read_admission_slots`, default 16. The read pool equals that value; readiness
keeps its separate connection. Python capacity is unchanged.

Plan review: architecture, quality, coverage, and performance PASS. Keep
`pool_max` as the checked total (slots + 1) for existing budget artifacts. Keep
the environment digest and release binding. Refuse zero, arithmetic overflow,
pool mismatch, and budgets above configured or observed PostgreSQL limits.
Do not change cancellation/connection ownership in `runtime/admission.rs`.

1. Add failing runtime/configuration and budget-helper tests. Run through the
   test-host helper. Record RED before implementation.
2. Implement Rust-owned configuration in `rs/read/src/lib.rs` and
   `runtime/{config,db}.rs`; update `scripts/prepare_rust_budget.py`. Update
   fixtures and admission cases to use configured slots. Run targeted checks;
   commit the complete configuration change.
3. Extend the existing measurement harness only as needed for a Rust-only
   small-workload probe. Use release binaries; offer 5, 10, 15, 20, 30, and
   40 RPS for 60 seconds each at 8 and 16 slots. Add 24 if 16 has increasing
   throughput without busy responses. Record successes, 503, p95, test-slice
   CPU, and available pool-wait evidence; do not infer unmeasured waits.
4. Update current AD-02/AD-03 documentation and report generation. Preserve
   historical measurements as historical, not new default assertions.
5. Run CI commands from `.github/workflows/rust-read-ci.yml`: workspace tests,
   fmt check, strict clippy, contract meta-tests, disposable contract and gate
   checks. Record each exit code separately. Review the diff and commit evidence.

## Execution boundary

All builds/tests use `verdaxis-prod-test "$PWD"` and
`/home/verdaxis-test/bin/heavy run 3600 -- bash -lc '...'`.
Measurements use `heavy measure 3600`. One call at a time, no subagents,
no live deployment, no push. Long calls write `work/` logs ending in
`CALL_EXIT=...`; wait for completion. Stop if the wrapper reports PAUSED.
Final evidence belongs here; temporary logs remain uncommitted in `work/`.

## Results

Configuration committed as `16a9486d`. RED: Rust runtime test exit 101
(refused one configured slot); budget helper exit 1 (fixed-nine rejection).
GREEN: workspace exit 0; full disposable CI contract command exit 0, all 11
groups passed. Deployment/budget and contract meta-tests: 588 passed, exit 0.

### Capacity probe

Exclusive `heavy measure 3600`, release profile, source `16a9486d`, existing
small workload and equal-weight route/tool schedule. One Rust service, Caddy,
and one disposable PostGIS database; no Python API service. Each offered window
is 60 seconds, plus response drain; 15-second warm-up per setting. p95 below
covers successful responses only. CPU is average occupied cores for the entire
3-CPU test slice, not percent of one core. Raw samples remain in the test lane's
`work/admission-probe/results.json`; committed summaries are in
`admission-capacity-results.json`.

| Slots | Offered RPS | Successes | Busy/503 | 429 | Success RPS incl. drain | p95 ms | Slice CPU cores |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 8 | 5 | 299 | 0 | 1 | 4.98 | 694 | 0.84 |
| 8 | 10 | 590 | 1 | 9 | 9.80 | 929 | 1.63 |
| 8 | 15 | 870 | 12 | 18 | 14.39 | 1019 | 2.23 |
| 8 | 20 | 1097 | 69 | 34 | 18.09 | 1084 | 2.70 |
| 8 | 30 | 1191 | 574 | 35 | 19.49 | 1564 | 2.96 |
| 8 | 40 | 1431 | 917 | 52 | 23.56 | 1426 | 2.96 |
| 16 | 5 | 299 | 0 | 1 | 4.98 | 687 | 0.81 |
| 16 | 10 | 591 | 0 | 9 | 9.82 | 897 | 1.64 |
| 16 | 15 | 879 | 2 | 19 | 14.50 | 1145 | 2.35 |
| 16 | 20 | 1045 | 128 | 27 | 17.21 | 2628 | 2.95 |
| 16 | 30 | 1373 | 376 | 51 | 22.44 | 2796 | 2.98 |
| 16 | 40 | 1403 | 949 | 48 | 22.94 | 3048 | 2.99 |

Highest tested **503-clean** rate: 8 slots = 5 RPS; 16 slots = 10 RPS.
No tested step is all-2xx: existing per-client price limits returned 429. These
are not gate overload, and remain enabled. No other HTTP status was observed.
The 24-slot condition was not met: 16 slots already returned busy at 15 RPS.

Recommendation: retain **16** as the default. It removes the premature 8-slot
limit and improves the 503-clean tested rate, but it is not a throughput cure.
At 20–40 RPS the slice reaches its 3-CPU cap and larger admission worsens p95.
Rust itself uses only about 0.34–0.35 cores at high load. With no Python service,
this points to PostgreSQL/query CPU as the main high-load cost; database-only CPU
was not sampled separately, so this is an inference, not direct attribution.
At 10 RPS the old gate rejects while CPU has headroom: that is an admission
limit. At higher load the gate protects a CPU-constrained stack.

**Pool waits:** acquisition durations were not instrumented in this branch.
The read pool equals slots, and query tasks retain permits until connection
return/close, so no separate undersized pool is configured. This is not proof
of zero acquisition latency. The test user cannot read the service journal;
that read returned an insufficient-permissions message. Do not use the absent
journal evidence to claim zero slow acquisitions. A follow-up pool timing probe
is needed for a direct wait-time breakdown; this task does not change ownership
code to insert measurement hooks.

Probe command exit 0; helper call exit 0. Every started service and disposable
container was stopped by the harness. No 24-slot run or live deployment occurred.

### Verification ledger

| Gate | Own exit code | Evidence |
| --- | --- | --- |
| `cargo test --manifest-path rs/Cargo.toml --locked --workspace` | 0 | `work/admission-acceptance.log` |
| `cargo fmt --manifest-path rs/Cargo.toml --all -- --check` | 0 | same |
| Strict workspace/all-targets clippy, `-D warnings` | 0 | same |
| `pytest tests/rust_contract -q` | 0 | 536 passed, five existing warnings |
| Budget, deploy, unit-install tests plus contract meta-tests | 0 | 588 passed; `work/admission-recheck.log` |
| `cargo test ... -p verdaxis-contract --test gate` | 0 | `work/admission-gate-final.log` |
| Runtime and admission PostgreSQL targets, `contract-tests`, serial | 0 | nine tests; `work/admission-postgres-final.log` |
| Full CI disposable contract command, all targets, mixed Caddy, complete ledger | 0 | all 11 groups; `work/admission-contract.log` |
| `scripts/rust_contract/sqlx_offline_check.py` | 0 | `work/admission-postgres-final.log` |
| Exclusive release capacity probe | 0 | `work/admission-probe.log` |
| `git diff --check` | 0 | local source-only check |

Earlier diagnostic failures are not hidden: fmt initially returned 1 (formatting
then applied on the host); an incorrect Python test filename returned 4;
an attempted `--test frozen_gate` returned 101 because no such target exists
(the actual `gate` target was then run). A final combined call lost SSH with
exit 255; its remote unit was allowed to finish naturally, status 0, and the
checks were rerun to recover each command's own status. No call was interrupted.
The configured-pool timezone fixture initially returned 101 because it held only
eight connections; corrected to hold all configured slots, then all nine focused
PostgreSQL tests passed. The broad legacy `run_postgres.sh` call returned 101 because
four market-matrix tests require their separate seeded database URL, which that
script does not supply. The full CI disposable contract command passed; focused
runtime/admission PostgreSQL checks are run separately with the same disposable
setup, not counted as a pass for the broad legacy command.

## Resume point

Implementation and listed gates complete. Remaining evidence gap: direct pool
acquisition timing was not measured. The 503-clean capacity result excludes
existing 429 rate limits; it is not an all-success-rate claim. No GitHub CI was
started because the task explicitly forbids pushing. PM owns push and CI.
No lane service remains running. No deployment or production performance approval.
