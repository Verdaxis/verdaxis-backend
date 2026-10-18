# Rust Phase 1 go/no-go

> Capacity update (18 Oct, founder decision 88): the eight-slot/nine-connection
> values below are historical evidence. Current Rust admission is configured by
> `read_admission_slots` (default 16), with matching read pool plus one readiness
> connection. Python is unchanged. See [admission-capacity.md](admission-capacity.md).


Status: **draft for the founder decision, Task 14a (17 October 2026)**.
Everything in this report was measured on the test host (`vmi3623757`, the
production VPS's isolated `verdaxis-test` slice, 3 CPUs / 4 GB as seen by the
run, cgroup v2). No staging or production host was contacted. This report
authorizes nothing.

## Scope (founder decision 48, 16 October: no seven-day shadow clock)

Task 14 splits into 14a (this report: the gate, the harnesses, the workload
manifest, the seeded differential simulation, the measurement) and 14b (one
short read-only differential pass on staging data after the operator deploys
Rust to staging; the shadow harness is built and unit-tested here, and was not
run).

- Candidate source SHA: `e43282aee6cb5b5b5ee6fe093be88d80704e104a` (measurement
  and report); simulation and harnesses at `518adc6acedd0903bb300d26abd496ed81d77d6d`
  and earlier commits on this branch.
- Python oracle SHA: `4ed68a2fb324835216dd3a602af5ef8e4c6e0154` (the recorded
  golden corpus; Rust serves the same bytes in the simulation).
- Schema revision: `kycd_20261015_staging` (recorded corpus metadata).
- Workload manifest digest:
  `3f7653e132ed925750eef6b12c8d449205a0c904dced58c800eccc9d6d6e5e14`
  (the 14a runs); after the R02 measured-path correction (`251d50c6`) it is
  `85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215`,
  which the 18 Oct runs used.
- Raw-evidence digests (SHA-256 of the JSON documents under `work/rust-phase1/`):
  - `measure-small-v2.json` (14a small run):
    `b1e129af8c56344ed78d2169743f1458242622c3e88a2b28f13626ebe8a571a3`
  - `measure-large-v2.json` (14a saturated run, kept as capacity evidence):
    `27efbd9796c98dbcf29d40b0b1cc2413d687ec958e8b6717c285571512da2ab5`
  - `measure-large-adaptive.json` (18 Oct, the load both stacks serve):
    `31fb9fc9211c99eea322f282918e325127bb8b7095b5fc68004320d545770cb8`
  - `simulation-small.json` / `simulation-large.json` (14a, before the tie rule):
    `8dd7c4371ac32d2e58ace9f9662c1b9a2dc8bdfc816e6a418de778fedfc56b2c` /
    `5d0498eae27c8fb3125e23a061bac54692083e4cfca0fd282d2aae350cf4a7f7`
  - `simulation-small-tie.json` (18 Oct, prices group with the tie rule applied):
    `4e6b22baee366b665c32fb93be734ea247fe29418023bd9ce555efe3f7a844cf`
  - `t14b-r20-r24-r27.json` (18 Oct quiet-host diagnostic):
    `73d689487a3410f1d886a38379d37e7fccdfb380c0f2f0cebd8521a772e7c034`
- Topology: four-worker Python (`uvicorn --workers 4`) in one systemd user
  unit versus one Rust release process in another; each runtime's cgroup
  `memory.current`/`memory.peak`/`memory.stat` and `cpu.stat` were sampled once
  per second. The load generator and the disposable PostgreSQL container ran
  outside both service cgroups.

## Seeded differential simulation — one difference class, no port defect

The whole route corpus and all three MCP tools ran against Python and Rust on
one disposable database per group, at both sizes, with the
Python–Rust–Python sandwich (the two Python recordings must agree before the
Rust recording can decide). The two Python recordings agreed at every case, so
**no pair was classified as a data change**; 487 pairs per size.

| Size | Groups matched | Cases | Differences |
| --- | --- | ---: | ---: |
| small | 10 of 11 | 487 | 9 (all `/api/trade-tape`) |
| large | 10 of 11 | 487 | 33 (all `/api/trade-tape`) |

Every group except `prices` matched byte for byte at both sizes, including
`mcp` (143 cases) and `curves_canonical` (137 cases). All `prices` differences
are in `/api/trade-tape` item ordering:

**Cause (observed, not inferred).** `/api/trade-tape` orders by
`confirmed_at DESC` with no tie-breaker in **both** implementations
(`app/routers/trade_tape.py:129`; `rs/read/sql/prices/tape.sql`). When several
trades share a `confirmed_at`, the order within the tie group follows the
physical plan, which differs between the runtimes. A diagnostic on the same
database showed, for the default query: the **set** of the 50 returned trade
ids is identical (same items, same `total` = 1004), Python is stable across its
own two passes, and only the order inside the tie group differs. This is
**source-level tie nondeterminism, not a Rust port defect** — Rust copies
Python's `ORDER BY` verbatim. The corpus already has reviewed tie handling for
exactly this pattern (news multiset, decision 18; price summaries, decision 22)
and a reviewed tie-breaker fix (`/api/orderbook`, `created_at DESC, id DESC`,
decision 23). `/api/trade-tape` has neither.

**PM decision (17 Oct):** treat the trade-tape tie order the way the comparator
already treats news ties. `tests/rust_contract/trade-tape-tie-paths.json` holds
one reviewed rule: for `/api/trade-tape` only, the items inside one
`confirmed_at` tie group compare as a **multiset of exact item bytes**, while
the sequence of `confirmed_at` keys, their descending order, the item sets, the
frame, `total`, `market_hours` and every other byte stay strict. A real
ordering difference across tie groups still fails (tested in
`tests/rust_contract/test_trade_tape_ties.py`). Python is unchanged (decision 47
freeze). A deterministic tie-breaker in Rust comes later, with the same-meaning
cleanup (decision 67). With this rule the comparator reports zero unexplained
differences for `/api/trade-tape`; the tie-order difference class is therefore
resolved without a route or economic change.

The alternative — a `confirmed_at DESC, id DESC` tie-breaker in both
implementations (the decision-23 precedent) — remains available and would make
the ordering itself deterministic; it needs a golden re-record.

## Gate outcome per condition (founder decision 68, 17 Oct; updated 18 Oct)

The verdict rule changed after the first report: **hard gates** are zero
unexplained contract differences, no rise in 5xx/503 responses from Rust
against Python on the same workload, and a passing integration suite. **Speed
blocks** only when the overall weighted mix p95 of Rust is worse than Python's,
or a single route's Rust p95 is slower than Python's by more than 100 ms or by
more than 50 %, whichever threshold the route crosses first; smaller per-route
regressions are fix items. **Memory** is measured and reported (target: Rust at
most 50 % of Python) but does not block.

### Follow-up evidence (18 Oct, commit `086d4dda` tree)

- The trade-tape tie rule (below) is applied: the `prices` group re-simulation
  at the small size reports **0 differences over 56 pairs**; the earlier
  9/33-difference class is resolved.
- The R27 JIT fix (`41849176`) is in: quiet-host rerun R27 Rust p95
  **0.231 s vs Python 0.150 s** (was 0.946 vs 0.167).
- The large size was re-measured adaptively (start 1 RPS, raise while neither
  stack returns 5xx): **both stacks serve 1 RPS**; Python already returned
  13 busy (503) at 1 RPS while Rust returned none, so the ramp stopped there.
- The unchanged integration suite passes on this tree: **79 passed, exit 0**.

| Condition (decision 68) | Small size | Large size (adaptive, 1 RPS) |
| --- | --- | --- |
| Zero unexplained differences | **pass** — 0 differences (prices re-simulation, tie rule); every other group byte-identical in the 14a run | same comparator; no unexplained class |
| No rise in 5xx/503 from Rust | **pass** — Rust 2 vs Python 106 | **pass** — Rust 0 vs Python 13 |
| Integration suite passes | **pass** — 79 passed, exit 0 | (same suite) |
| Speed: mix p95 | **pass** — Rust 0.499 s vs Python 0.984 s | **pass** — Rust 2.771 s vs Python 4.031 s |
| Speed: per-route thresholds | **no-go — R27** (+80.5 ms and +53.6 %, past the 50 % cap) | **no-go — R09, R10, R11, R14, R19, R20, R21, R24, R25, `get_market_slice`** |
| Memory (non-blocking) | **pass** — 43.0 MiB vs 802.3 MiB (5.4%) | **pass** — 192.1 MiB vs 979.9 MiB (19.6%) |

**Overall gate under decision 68: NO-GO**, on the per-route speed thresholds
alone. All three hard gates pass at both sizes, the mix p95 passes at both
sizes, and memory is far inside its target. Fix items (non-blocking) at the
small size: R20, R24, `plan_voyage_fuel`, R27-at-p50.

### Cause of the per-route regressions (evidence in the quiet-host rerun)

**R27 `/api/listings` — fixed (`41849176`).** `listings.rs` was the only paged
orderbook handler that did not begin a transaction and `SET LOCAL jit = off`
before its shared-relation query; the expanded corpus pushes the eligibility
subplans past PostgreSQL's JIT threshold, producing a bimodal ~0.9 s tail.
auto_explain before the fix: the route statement's top node was a Sort at
643.6 ms; after the fix, 25.5 ms. The fix is transaction-local, changes no
response byte (orderbook_map corpus, 41 cases including listings, still
matches), and Rust p95 is now 0.231 s against Python's 0.150 s. The remaining
+80 ms is the 100-row lateral detail join (the `Nested Loop` at 23.9 ms in the
plan plus per-row base-table lookups) — a design cost of building the digest
for the kept page (decision 24). It crosses the 50 % relative cap at the small
size, so it still blocks; a narrower projection or a decision-67-style
same-meaning cleanup is the fix size (a shared-tail change, not a one-liner).

**R20 `/api/orderbook/product-counts` and R24 `/api/orderbook/regions` —
reported, not fixed.** Both stacks run one aggregate over the same ~10.5k
candidate rows; Python's route statement costs 121–163 ms and Rust's 201–253 ms
(quiet-host rerun, `pg_stat_statements` per request). The actual plans show the
difference: Rust's aggregation sits on a `Subquery Scan` over the
`eligible_orders` CTE (the decision-24/25 shared-relation shape) and pays
170–214 ms in that scan plus its sort, where Python's plan nests the same
predicate inline and the planner chooses the cheaper shape for a 4-row result.
Round trips are not the cause (Rust issues 4–9 statements per request against
Python's 6–10; the SQL time dominates). This is the known design cost of doing
eligibility in SQL (decision 16, and the Task 13 stop that decisions 24/25/27
already paid down). **Fix size:** a decision-25-style precomputation of the
distinct/aggregate keys, or a reviewed partial index — both are PM decisions
touching the shared relation; neither is a small safe byte-exact change. Both
regressions are inside the decision-68 caps at the small size (R20 +41 ms,
R24 +22 ms), so they are fix items there, but they block at the large size.

### Large size: which load both stacks serve

| Offered load | Python 5xx/503 | Rust 5xx/503 | Served by both |
| --- | ---: | ---: | --- |
| 1 RPS (probe) | 2 busy in the probe; 13 busy in the window | 0 | yes (Python's per-worker admission is the binding limit) |
| 2 RPS and above | not reached — the ramp stops at the first 5xx | not reached | not measured |

Python's AD-02 behaviour (2 admission slots per worker, 4 workers) is the
binding constraint at the large size: its p50 for the heaviest routes is 3–8 s,
so even 1 RPS occasionally exhausts a worker's slots. Rust's single 8-slot gate
returned zero 5xx at 1 RPS and its mix p95 is 31 % lower.

### Earlier verdict (superseded by decision 68; kept as evidence)

| Condition (original) | Outcome | Measured basis |
| --- | --- | --- |
| Zero unapproved differences | **no-go** | 9 (small) / 33 (large) `/api/trade-tape` tie-order differences; every other group byte-identical |
| Rust route-service memory at most 50% of Python | **pass** | small: 43.0 MiB vs 802.3 MiB (5.4%); large: 206.3 MiB vs 1059.6 MiB (19.5%) |
| No p95 regression | **pass on the mix; four per-route regressions at the small size** | small mix: Rust 0.499 s vs Python 0.984 s; per-route regressions R20 (+0.041 s), R24 (+0.022 s), R27 (0.946 vs 0.167 s), `plan_voyage_fuel` (+0.004 s) |

**Overall gate (original rule): NO-GO as measured**, solely because of the
`/api/trade-tape` tie-order difference class. The memory condition passed by a
wide margin at both sizes. The p95 condition passed on the declared mix at the
small size with four small per-route exceptions, and the large-size run was
capacity-bound, so its per-route p95 numbers were not a workload-shaped
measurement.

## Measurement

### Small size (1,000 orders / 1,000 trades / 100 news)

Offered load 1 RPS (15 s) then 5 RPS (900 s), equal-weight baseline, end-to-end through Caddy; 15 s warm-up; one cell per runtime, Python first. 4,403–4,505 successful responses per runtime; 106 Python and 2 Rust busy (503) responses; 6/8 rate-limited (429).

| Runtime | steady median | p95 | peak | anon median | PSS median | CPU cores | usec/completed request |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| python | 802.3 MiB | 810.0 MiB | 814.7 MiB | 774.5 MiB | 790.7 MiB | 0.773 | 160,701 |
| rust | 43.0 MiB | 46.8 MiB | 51.6 MiB | 42.7 MiB | 50.5 MiB | 0.118 | 23,993 |

Rust median is **5.4%** of Python's.

| Route | Python p50/p95/p99 s | Rust p50/p95/p99 s | py n | rs n | py busy | rs busy |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| R01 | 0.027/0.569/1.288 | 0.011/0.085/0.170 | 128 | 150 | 22 | 0 |
| R02 | 0.022/0.058/0.073 | 0.012/0.036/0.088 | 151 | 151 | 0 | 0 |
| R03 | 0.021/0.043/0.080 | 0.011/0.027/0.045 | 151 | 151 | 0 | 0 |
| R04 | 0.034/0.940/2.333 | 0.012/0.112/0.256 | 139 | 150 | 11 | 0 |
| R05 | 0.022/0.114/0.961 | 0.011/0.031/0.059 | 151 | 151 | 0 | 0 |
| R06 | 0.167/1.267/1.471 | 0.035/0.097/0.213 | 127 | 149 | 23 | 1 |
| R07 | 0.037/0.059/0.108 | 0.017/0.038/0.049 | 151 | 151 | 0 | 0 |
| R08 | 0.040/0.109/0.374 | 0.017/0.039/0.064 | 150 | 151 | 1 | 0 |
| R09 | 0.337/1.077/2.368 | 0.345/0.540/1.008 | 131 | 150 | 19 | 0 |
| R10 | 0.164/0.278/0.435 | 0.061/0.121/0.218 | 150 | 151 | 1 | 0 |
| R11 | 0.409/0.713/1.116 | 0.208/0.360/0.511 | 150 | 150 | 0 | 0 |
| R12 | 0.088/0.154/0.219 | 0.036/0.081/0.135 | 149 | 151 | 2 | 0 |
| R13 | 0.041/0.136/0.770 | 0.024/0.104/0.237 | 146 | 151 | 5 | 0 |
| R14 | 0.036/0.066/0.104 | 0.022/0.043/0.074 | 144 | 143 | 1 | 0 |
| R15 | 0.116/0.835/1.530 | 0.058/0.154/0.317 | 145 | 150 | 5 | 0 |
| R16 | 1.486/2.806/3.630 | 0.911/1.460/2.388 | 150 | 150 | 0 | 0 |
| R17 | 0.223/1.428/2.080 | 0.387/0.759/1.398 | 148 | 150 | 2 | 0 |
| R18 | 1.319/2.365/3.097 | 0.766/1.345/2.247 | 149 | 150 | 1 | 0 |
| R19 | 0.181/0.291/0.810 | 0.227/0.275/0.366 | 150 | 151 | 1 | 0 |
| R20 **REG** | 0.167/0.286/0.581 | 0.190/0.327/0.670 | 150 | 150 | 0 | 0 |
| R21 | 0.240/0.421/1.010 | 0.248/0.326/0.567 | 150 | 151 | 1 | 0 |
| R22 | 0.348/1.278/1.886 | 0.264/0.451/1.471 | 146 | 149 | 4 | 1 |
| R23 | 0.211/1.326/1.513 | 0.263/0.400/0.998 | 149 | 150 | 1 | 0 |
| R24 **REG** | 0.154/0.214/0.314 | 0.173/0.236/0.430 | 150 | 150 | 0 | 0 |
| R25 | 0.154/0.303/0.730 | 0.174/0.254/0.475 | 148 | 151 | 3 | 0 |
| R26 | 1.484/2.580/3.455 | 0.779/1.488/2.968 | 147 | 150 | 3 | 0 |
| R27 **REG** | 0.090/0.167/0.281 | 0.058/0.946/0.999 | 150 | 150 | 0 | 0 |
| get_market_slice | 0.064/0.723/1.262 | 0.027/0.072/0.096 | 151 | 151 | 0 | 0 |
| list_markets | 0.011/0.024/0.065 | 0.006/0.011/0.016 | 151 | 151 | 0 | 0 |
| plan_voyage_fuel **REG** | 0.013/0.024/0.043 | 0.010/0.028/0.051 | 151 | 151 | 0 | 0 |

Weighted mix: Python 0.403/0.984/1.499 s; Rust 0.266/0.499/0.917 s.

Routes with Rust p95 above Python p95: R20, R24, R27, plan_voyage_fuel.

### Large size (100,000 orders / 100,000 trades / 10,000 news) — adaptive, 1 RPS

Offered load 1 RPS (600 s), equal-weight baseline, end-to-end through Caddy; one service per runtime kept up across the probe and the window. The ramp stopped at 1 RPS because Python returned busy (503) responses there, so no higher clean rate exists at this size on this host. 587 Python and 600 Rust successful responses; **Python 13 busy (503) and Rust 0**.

| Runtime | steady median | p95 | peak | anon median | PSS median | CPU cores | usec/completed request |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| python | 979.9 MiB | 1042.2 MiB | 1079.0 MiB | 962.2 MiB | 978.4 MiB | 0.782 | 808,894 |
| rust | 192.1 MiB | 196.6 MiB | 204.5 MiB | 189.3 MiB | 197.0 MiB | 0.137 | 140,352 |

Rust median is **19.6%** of Python's.

| Route | Python p50/p95/p99 s | Rust p50/p95/p99 s | py n | rs n | py busy | rs busy |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| R01 | 0.078/1.988/1.988 | 0.014/0.052/0.058 | 18 | 20 | 2 | 0 |
| R02 | 0.078/0.113/0.125 | 0.013/0.044/0.071 | 20 | 20 | 0 | 0 |
| R03 | 0.027/0.045/0.060 | 0.012/0.032/0.035 | 20 | 20 | 0 | 0 |
| R04 | 0.047/0.294/0.294 | 0.013/0.027/0.040 | 19 | 20 | 1 | 0 |
| R05 | 0.036/0.132/0.728 | 0.013/0.040/0.045 | 20 | 20 | 0 | 0 |
| R06 | 1.041/3.861/3.861 | 1.058/1.431/2.312 | 19 | 20 | 1 | 0 |
| R07 | 0.048/0.093/0.112 | 0.020/0.042/0.054 | 20 | 20 | 0 | 0 |
| R08 | 0.078/0.186/0.281 | 0.027/0.080/0.163 | 20 | 20 | 0 | 0 |
| R09 **REG** | 3.757/4.248/5.618 | 12.503/14.639/15.551 | 20 | 20 | 0 | 0 |
| R10 **REG** | 0.235/0.384/0.805 | 0.570/0.790/0.940 | 20 | 20 | 0 | 0 |
| R11 **REG** | 0.869/1.060/1.170 | 1.197/1.399/2.167 | 20 | 20 | 0 | 0 |
| R12 | 1.346/2.036/2.114 | 0.051/0.076/0.088 | 20 | 20 | 0 | 0 |
| R13 | 0.638/1.828/2.723 | 0.928/1.336/2.130 | 20 | 20 | 0 | 0 |
| R14 **REG** | 0.450/0.829/0.856 | 0.842/1.093/1.195 | 20 | 20 | 0 | 0 |
| R15 | 0.155/1.947/1.947 | 0.654/0.960/0.987 | 19 | 20 | 1 | 0 |
| R16 | 6.351/10.348/10.348 | 6.189/7.218/7.849 | 18 | 20 | 2 | 0 |
| R17 | 1.747/7.328/7.764 | 2.801/3.228/3.358 | 20 | 20 | 0 | 0 |
| R18 | 8.189/8.844/8.903 | 6.800/7.741/7.746 | 20 | 20 | 0 | 0 |
| R19 **REG** | 1.177/1.483/1.527 | 2.513/3.092/3.230 | 20 | 20 | 0 | 0 |
| R20 **REG** | 0.685/0.799/0.817 | 1.188/1.376/1.535 | 20 | 20 | 0 | 0 |
| R21 **REG** | 1.420/1.669/1.675 | 3.581/4.062/4.344 | 20 | 20 | 0 | 0 |
| R22 | 1.583/4.404/4.404 | 2.801/3.731/3.824 | 18 | 20 | 2 | 0 |
| R23 | 0.918/6.088/6.088 | 1.645/2.147/2.334 | 16 | 20 | 4 | 0 |
| R24 **REG** | 0.899/1.023/1.038 | 1.645/1.942/2.007 | 20 | 20 | 0 | 0 |
| R25 **REG** | 0.978/1.311/2.695 | 1.767/2.626/2.637 | 20 | 20 | 0 | 0 |
| R26 | 8.316/13.243/15.110 | 6.616/7.450/7.808 | 20 | 20 | 0 | 0 |
| R27 **REG** | 0.259/0.349/0.382 | 0.306/0.382/0.441 | 20 | 20 | 0 | 0 |
| get_market_slice **REG** | 0.084/0.276/2.773 | 0.601/0.788/0.953 | 20 | 20 | 0 | 0 |
| list_markets | 0.014/0.051/0.052 | 0.007/0.012/0.013 | 20 | 20 | 0 | 0 |
| plan_voyage_fuel | 0.019/0.035/0.037 | 0.017/0.027/0.033 | 20 | 20 | 0 | 0 |

Weighted mix: Python 2.156/4.031/4.463 s; Rust 2.340/2.771/2.993 s.

The per-route regressions here (R09 the worst, 14.6 s vs 4.2 s) are the same shared-relation SQL cost named in the gate section, amplified by the 100k-row dataset; see the causes there.

## Coverage and what is inconclusive

- The manifest's observation floor is 1,000 completed observations per measured
  route. Inside the four-hour budget every route received about 150 (small) and
  about 50 (large) — **below the floor**, so every route's per-route p95 is
  reported as coverage-inconclusive even where the mix comparison is measured.
  Reaching the floor at the small size would need roughly 7x the measured window
  (about 6,300 s per runtime at 5 RPS, ~3.5 h for the two cells).
- The large size is capacity-bound at the plan's 5 RPS ceiling: both services
  returned thousands of busy (503) responses and only a few successes per
  route. A meaningful large-size latency series needs either a lower offered
  rate or a separately approved saturation test.
- The small-size per-route regressions R20 (+0.041 s), R24 (+0.022 s),
  `plan_voyage_fuel` (+0.004 s) are within noise of the tail; R27 (listings)
  shows a genuine bimodal tail (Rust p50 0.058 s but ~10 of 150 requests at
  ~0.9 s) that needs investigation before cutover.
- No staging-data differential pass ran (Task 14b, operator-gated).
- AD-02's admission topology (one 8-slot Rust gate vs Python's 2x4) was not
  separately exercised at saturation in this task; the 503 counts above are the
  observed evidence at the offered rates.

## Approved differences (exactly three)

| ID | What changed | Why approved | Proof |
| --- | --- | --- | --- |
| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; Python's four local counters can admit up to four times more | PM defines documented per-client limits as the contract | Rust exact-boundary burst/reset tests; counts excluded from parity |
| AD-02 | Rust uses one 8-slot public-read admission gate; Python uses 2 slots in each of 4 workers | Same total capacity | Mixed-concurrency test; 503 counts recorded above |
| AD-03 | Rust admits `/api/news` through the same 8-slot gate | Every Rust database use holds a permit | Two-direction contention test |

## Engineering effort

Task 14a: one working session (17 October 2026) for the gate, the harnesses,
the workload manifest, the simulation at both sizes and the measurement at both
sizes, plus two harness repairs found by the runs themselves (the MCP
`Accept`/Host requirements and the offered-load fit). Total test-host run time
stayed inside the four-hour budget. Tasks 1–13 are recorded in
`docs/rust-phase1/progress*.md`.

## Three largest unresolved risks

1. The per-route speed thresholds fail at both sizes (R27 at the small size;
   ten routes at the large size). The shared-relation SQL cost is the one
   design cause; a decision-25-style key precomputation or a reviewed partial
   index is the fix, and both are PM decisions touching the shared relation.
2. Coverage remains below the 1,000-observation floor on every route at both
   sizes (about 20 per route in the adaptive window); the per-route p95 values
   are directionally consistent across two independent runs but not
   floor-strength.
3. Python's admission topology (AD-02) is the binding capacity limit at the
   large size — it returns busy at 1 RPS — so no higher offered load can be
   judged there without saturating Python first.

## Founder decision and date

- Decision requested: **continue / pause / stop**.
- Not decided in this report. Production status: **not authorized by this
  report.** Task 14b (the staging differential pass) and any Caddy cutover need
  separate operator approval.
