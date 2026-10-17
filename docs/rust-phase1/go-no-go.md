# Rust Phase 1 go/no-go

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
  `3f7653e132ed925750eef6b12c8d449205a0c904dced58c800eccc9d6d6e5e14`.
- Raw-evidence digests (SHA-256 of the JSON documents under `work/rust-phase1/`):
  - `measure-small-v2.json`:
    `b1e129af8c56344ed78d2169743f1458242622c3e88a2b28f13626ebe8a571a3`
  - `measure-large-v2.json`:
    `27efbd9796c98dbcf29d40b0b1cc2413d687ec958e8b6717c285571512da2ab5`
  - `simulation-small.json`:
    `8dd7c4371ac32d2e58ace9f9662c1b9a2dc8bdfc816e6a418de778fedfc56b2c`
  - `simulation-large.json`:
    `5d0498eae27c8fb3125e23a061bac54692083e4cfca0fd282d2aae350cf4a7f7`
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

**Required decision before a pass can be claimed:** add the tie-breaker to both
implementations (the decision-23 precedent, permitted by the Python freeze as a
user-facing pagination fix) or add a trade-tape tie exception, then re-record
the golden corpus. This task changed no route code and no economic code.

## Gate outcome per condition (founder decision 68, 17 Oct)

The verdict rule changed after the first report: **hard gates** are zero
unexplained contract differences, no rise in 5xx/503 responses from Rust
against Python on the same workload, and a passing integration suite. **Speed
blocks** only when the overall weighted mix p95 of Rust is worse than Python's,
or a single route's Rust p95 is slower than Python's by more than 100 ms or by
more than 50 %, whichever threshold the route crosses first; smaller per-route
regressions are fix items. **Memory** is measured and reported (target: Rust at
most 50 % of Python) but does not block. The earlier verdict table is kept
below unchanged as evidence; only the verdict logic changed.

| Condition (decision 68) | Outcome | Measured basis |
| --- | --- | --- |
| Zero unexplained contract differences | **no-go** | 9 (small) / 33 (large) `/api/trade-tape` tie-order differences; every other group byte-identical |
| No rise in 5xx/503 from Rust against Python | **pass at the small size; unmeasured at the large size** | small: Rust 2 busy/0 other 5xx vs Python 106 busy (Rust returned fewer); large: both saturated, so no comparable unsaturated count exists yet |
| The integration suite passes | **pass** (recorded at this tree's Task 13/13b gates) | the unchanged disposable integration suite passed in the Task 13b combined gate |
| Speed: mix p95 | **pass** | small mix: Rust 0.499 s vs Python 0.984 s |
| Speed: per-route thresholds | **no-go — R27 (`/api/listings`)** | R27 0.946 vs 0.167 s (+779 ms, +467 %) is beyond both caps. R20 (+0.041 s), R24 (+0.022 s) and `plan_voyage_fuel` (+0.004 s) are inside both caps: fix items, not blocks |
| Memory target (non-blocking) | **pass** | small: 43.0 MiB vs 802.3 MiB (5.4%); large: 206.3 MiB vs 1059.6 MiB (19.5%) |

**Overall gate under decision 68: NO-GO**, on the `/api/trade-tape` tie-order
difference class and R27's per-route regression. R20, R24 and `plan_voyage_fuel`
are fix items. The tie class is removed separately by the PM's tie-group rule
below; R27's cause and fix are under investigation.

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

### Large size (100,000 orders / 100,000 trades / 10,000 news)

Same schedule. At this size 5 RPS exceeds both services' capacity: 2,580 Python and 3,211 Rust busy (503) responses and 46 Rust 500s; per-route success counts are small, so the p95 cells below are capacity-bound, not a workload-shaped latency measurement. Memory and CPU are steady-state and comparable.

| Runtime | steady median | p95 | peak | anon median | PSS median | CPU cores | usec/completed request |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| python | 1059.6 MiB | 1136.3 MiB | 1200.4 MiB | 1054.5 MiB | 1070.8 MiB | 1.301 | 618,174 |
| rust | 206.3 MiB | 220.2 MiB | 225.1 MiB | 195.8 MiB | 203.5 MiB | 0.090 | 66,155 |

Rust median is **19.5%** of Python's.

| Route | Python p50/p95/p99 s | Rust p50/p95/p99 s | py n | rs n | py busy | rs busy |
| --- | --- | --- | ---: | ---: | ---: | ---: |
| R01 | 0.162/0.499/1.103 | 0.051/0.232/0.232 | 49 | 19 | 101 | 131 |
| R02 | 0.118/1.271/5.127 | 0.075/0.275/0.953 | 54 | 59 | 97 | 92 |
| R03 | 0.075/2.129/3.161 | 0.021/0.106/0.771 | 50 | 39 | 101 | 112 |
| R04 | 0.072/2.189/5.723 | 0.027/0.206/0.206 | 50 | 15 | 100 | 135 |
| R05 | 0.099/5.797/9.931 | 0.023/0.072/0.072 | 151 | 17 | 0 | 134 |
| R06 | 2.396/5.910/17.141 | 2.797/4.509/4.634 | 41 | 21 | 109 | 129 |
| R07 **REG** | 0.122/0.375/0.562 | 0.045/0.532/0.544 | 46 | 38 | 105 | 113 |
| R08 | 0.165/2.506/11.600 | 0.044/0.419/0.419 | 44 | 19 | 107 | 132 |
| R09 **REG** | 7.779/20.401/30.167 | 29.879/33.946/33.946 | 55 | 14 | 95 | 136 |
| R10 **REG** | 0.643/2.139/15.911 | 1.360/2.225/3.215 | 58 | 67 | 93 | 84 |
| R11 | 2.476/11.326/12.996 | 3.288/4.844/4.967 | 59 | 39 | 91 | 111 |
| R12 | 4.579/13.755/19.695 | 0.131/1.100/3.335 | 52 | 26 | 99 | 125 |
| R13 | 1.564/8.255/9.951 | 2.835/4.957/7.468 | 48 | 22 | 103 | 129 |
| R14 | 1.844/4.154/8.003 | 3.103/4.128/5.190 | 61 | 48 | 90 | 103 |
| R15 **REG** | 0.413/2.578/4.385 | 2.437/5.777/5.777 | 49 | 16 | 101 | 134 |
| R16 | 14.361/24.970/27.049 | 16.973/22.326/27.133 | 38 | 23 | 112 | 127 |
| R17 **REG** | 4.594/9.495/17.546 | 8.830/12.470/12.470 | 39 | 19 | 111 | 131 |
| R18 | 19.029/33.096/36.667 | 19.304/25.811/27.263 | 45 | 33 | 105 | 117 |
| R19 **REG** | 2.605/11.180/22.918 | 7.827/11.929/12.766 | 52 | 37 | 99 | 114 |
| R20 | 1.964/8.825/12.464 | 4.111/7.666/9.746 | 49 | 38 | 101 | 112 |
| R21 **REG** | 4.863/9.677/13.410 | 11.158/14.371/14.371 | 57 | 19 | 94 | 132 |
| R22 **REG** | 4.812/14.410/14.796 | 11.256/15.919/17.179 | 54 | 25 | 96 | 125 |
| R23 **REG** | 2.807/6.707/16.824 | 5.427/10.410/10.410 | 31 | 18 | 119 | 132 |
| R24 | 2.517/11.416/15.323 | 6.602/10.123/10.546 | 79 | 78 | 71 | 72 |
| R25 **REG** | 2.812/7.381/14.560 | 5.855/9.350/9.408 | 76 | 38 | 75 | 113 |
| R26 | 19.320/28.839/38.740 | 19.218/26.264/26.264 | 46 | 14 | 104 | 136 |
| R27 **REG** | 0.798/6.188/10.328 | 26.750/29.322/29.322 | 49 | 4 | 101 | 100 |
| get_market_slice | 0.103/5.964/9.964 | 0.009/1.808/3.842 | 151 | 151 | 0 | 0 |
| list_markets | 0.060/1.299/4.908 | 0.008/0.053/0.641 | 151 | 151 | 0 | 0 |
| plan_voyage_fuel | 0.063/7.059/10.911 | 0.013/0.129/0.944 | 151 | 151 | 0 | 0 |

Weighted mix: Python 5.261/11.576/16.768 s; Rust 7.667/10.721/11.697 s.

Routes with Rust p95 above Python p95: R07, R09, R10, R15, R17, R19, R21, R22, R23, R25, R27.

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

1. The `/api/trade-tape` tie order needs a reviewed decision and a golden
   re-record before any zero-difference claim.
2. R27 (`/api/listings`) shows a bimodal Rust tail at the small size; the cause
   is unknown and it is the one route whose p95 exceeded Python's by 5.7x.
3. The large size exceeds both runtimes' capacity at 5 RPS, so no
   workload-shaped large-size latency comparison exists yet.

## Founder decision and date

- Decision requested: **continue / pause / stop**.
- Not decided in this report. Production status: **not authorized by this
  report.** Task 14b (the staging differential pass) and any Caddy cutover need
  separate operator approval.
