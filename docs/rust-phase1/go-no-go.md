# Rust Phase 1 go/no-go

## Decision 82 — 18 October 2026

**Overall: NO-GO. No deployment is authorized.** Decision 82 changes large-size
speed from blocking to reporting. Small-size speed still blocks, and the
retained large-size contract result still prevents GO.

### Rule (founder decisions 68 and 82)

- Hard gates block at **both** sizes: zero unexplained contract differences,
  no rise in Rust 5xx/503 against Python on the same workload, and a passing
  integration suite.
- **Small:** overall weighted p95 must not rise. A route regression blocks
  only above 100 ms or above 50%; exact boundaries do not block.
- **Large:** overall weighted p95 is reported, not blocking. Every route
  regression is a fix item for the same-meaning pass, not a blocker. Production
  data will not reach this size before that pass is live (founder rationale).
- Complete valid measurement evidence is still mandatory at both sizes.
  Missing or invalid evidence gives INCONCLUSIVE, never GO. Memory is reported.

### Evidence and verdict

- The small `prices` rerun has zero differences. It replaces only the small
  `prices` result, not the large result. The retained large simulation still
  reports 33 differences. The reviewed tie rule explains that historical class,
  but there is no large-size rerun under that rule in the supplied evidence.
  The report must not infer zero from the small rerun. The generated contract
  gate below conservatively retains the recorded 33 differences (NO-GO);
  current large-size zero-difference evidence remains unmeasured.
- Error gate: PASS at both measured sizes. Offered totals match per route:
  small 4,515 requests per stack, Rust 2/4,515 versus Python 106/4,515;
  large 600 per stack, Rust 0/600 versus Python 13/600. A Python-only busy
  response does not block decision 68.
- Adaptive selection: **no clean rate found**. The first 1 RPS probe returned
  Python 2 busy responses and Rust 0. The old harness nevertheless measured a
  600-second window at 1 RPS. Those samples are retained as historical evidence,
  not a clean-rate measurement. No higher rate was tested.
- Small speed remains NO-GO on R27. Large speed is now non-blocking; its
  former blocking routes are listed below as same-meaning fix items. Mix p95
  is lower for Rust at both sizes. These are the
  existing samples, not a new measurement of this gate-only branch. The small
  R27 diagnostic after the JIT fix is separate from the full workload samples.
- Integration: historical evidence says 79 passed, exit 0. This lane did not
  rerun the mutating integration suite. The generated report carries that
  explicit historical assertion, not a new integration claim.
- Memory is non-blocking. The manifest observation floor remains unmet; the
  generated coverage section lists that limitation. Staging Task 14b is not run.

The following sections were regenerated on the isolated test host from copied
JSON evidence. The sibling worktree and its evidence were not modified.
Later same-size/group simulation runs replace only their exact pair; missing
files, counts, groups or sizes are INCONCLUSIVE, never GO.

### Fix items for the same-meaning pass

- Large routes above the old blocking thresholds: **R09, R10, R11, R14, R19,
  R20, R21, R24, R25, `get_market_slice`**. Decision 82 makes all ten fix items.
- Large R27 is also a fix item (a smaller regression).
- Small non-blocking fix items: R20, R24 and `plan_voyage_fuel`.
- Small R27 remains a blocker in the full-workload evidence, not merely a fix item.

## Verification of the gate-only changes

All builds and tests ran through `verdaxis-prod-test`, through `/home/verdaxis-test/bin/heavy run 3600`
for suites and Rust commands. No SQL or serving-route code changed.

| Check | Exit code |
| --- | ---: |
| Rust gate tests (25 passed) | 0 |
| `tests/rust_contract` (590 passed; 5 dependency deprecation warnings) | 0 |
| `cargo test --manifest-path rs/Cargo.toml --locked --workspace` | 0 |
| `cargo fmt --manifest-path rs/Cargo.toml --all -- --check` | 0 |
| `cargo clippy --manifest-path rs/Cargo.toml --locked --workspace --all-targets -- -D warnings` | 0 |
| Report regeneration (file existence checked) | 0 |

## Regenerated evidence

# Rust Phase 1 go/no-go

Generated from existing evidence; small and large workloads are decided separately.

## Small workload

- Measurement file SHA-256: b1e129af8c56344ed78d2169743f1458242622c3e88a2b28f13626ebe8a571a3
- Measured source SHA: e43282aee6cb5b5b5ee6fe093be88d80704e104a

# Rust Phase 1 go/no-go

- Decision requested: continue / pause / stop.
- Candidate source SHA: 3c6ca84d41f2673e8c8d2f6b929e4e5e62c9d29a
- Python oracle SHA and corpus digest: 4ed68a2fb324835216dd3a602af5ef8e4c6e0154 / 0c0d252f5c23fcfd3cc3d051d04077d981601f1433575cf0197cfcaee7ec9060
- Schema revision and policy digest: kycd_20261015_staging / c935b83ed8f77faf74ced70849e10447ca351f9f3c1485f243b1cca36bf09693
- Rust toolchain, target, libc minimum, Cargo.lock digest, artifact digest: not measured
- Host/CPU/cgroup/PostgreSQL/PostGIS configuration: not measured
- Scope: 27 data GETs, 3 health GETs, 3 MCP transport pairs; 3 MCP tools.

## Contract and simulation

- Contract result: not measured cases, not measured passed, not measured failed, not measured skipped; not measured unapproved differences.
- Contract evidence digest: not measured
- Seeded differential simulation: size small: 10/11 groups matched, 487 pairs, 9 differences, gate no-go; size large: 10/11 groups matched, 487 pairs, 33 differences, gate no-go; size small: 1/1 groups matched, 56 pairs, 0 differences, gate pass
- Simulation evidence digest: 9e08738dc84b10da0b71dd69e8a61277762454afadbabdf87c8b8e7c069fc426
- Workload manifest digest: 85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215
- Workload sizes: {'small': {'orders': 1000, 'trades': 1000, 'news': 100}, 'large': {'orders': 100000, 'trades': 100000, 'news': 10000}}

## Memory

| Runtime | steady median | p95 | peak | anon median | file median | PSS median | ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| python | 802.3 MiB | 810.0 MiB | 814.7 MiB | 774.5 MiB | 23.9 MiB | 790.7 MiB |  |
| rust | 43.0 MiB | 46.8 MiB | 51.6 MiB | 42.7 MiB | 0.0 MiB | 50.5 MiB | 0.05 |

## CPU

| Runtime | core-equivalents | usec / completed request |
| --- | ---: | ---: |
| python | 0.7733298921813209 | 160701.04860322506 |
| rust | 0.11823090938269654 | 23992.829522752498 |

## Latency (end-to-end through Caddy)

The latency series is successful (2xx) responses only. Busy (503), rate-limited (429), contract 4xx and transport failures are completed observations reported as their own counts.

| Route | Python p50/p95/p99 | Rust p50/p95/p99 | py n | rs n | py busy | rs busy | py 429 | rs 429 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| R01 | 0.027/0.569/1.288 | 0.011/0.085/0.170 | 128 | 150 | 22 | 0 | 0 | 0 |
| R02 | 0.022/0.058/0.073 | 0.012/0.036/0.088 | 151 | 151 | 0 | 0 | 0 | 0 |
| R03 | 0.021/0.043/0.080 | 0.011/0.027/0.045 | 151 | 151 | 0 | 0 | 0 | 0 |
| R04 | 0.034/0.940/2.333 | 0.012/0.112/0.256 | 139 | 150 | 11 | 0 | 0 | 0 |
| R05 | 0.022/0.114/0.961 | 0.011/0.031/0.059 | 151 | 151 | 0 | 0 | 0 | 0 |
| R06 | 0.167/1.267/1.471 | 0.035/0.097/0.213 | 127 | 149 | 23 | 1 | 0 | 0 |
| R07 | 0.037/0.059/0.108 | 0.017/0.038/0.049 | 151 | 151 | 0 | 0 | 0 | 0 |
| R08 | 0.040/0.109/0.374 | 0.017/0.039/0.064 | 150 | 151 | 1 | 0 | 0 | 0 |
| R09 | 0.337/1.077/2.368 | 0.345/0.540/1.008 | 131 | 150 | 19 | 0 | 0 | 0 |
| R10 | 0.164/0.278/0.435 | 0.061/0.121/0.218 | 150 | 151 | 1 | 0 | 0 | 0 |
| R11 | 0.409/0.713/1.116 | 0.208/0.360/0.511 | 150 | 150 | 0 | 0 | 0 | 0 |
| R12 | 0.088/0.154/0.219 | 0.036/0.081/0.135 | 149 | 151 | 2 | 0 | 0 | 0 |
| R13 | 0.041/0.136/0.770 | 0.024/0.104/0.237 | 146 | 151 | 5 | 0 | 0 | 0 |
| R14 | 0.036/0.066/0.104 | 0.022/0.043/0.074 | 144 | 143 | 1 | 0 | 6 | 8 |
| R15 | 0.116/0.835/1.530 | 0.058/0.154/0.317 | 145 | 150 | 5 | 0 | 0 | 0 |
| R16 | 1.486/2.806/3.630 | 0.911/1.460/2.388 | 150 | 150 | 0 | 0 | 0 | 0 |
| R17 | 0.223/1.428/2.080 | 0.387/0.759/1.398 | 148 | 150 | 2 | 0 | 0 | 0 |
| R18 | 1.319/2.365/3.097 | 0.766/1.345/2.247 | 149 | 150 | 1 | 0 | 0 | 0 |
| R19 | 0.181/0.291/0.810 | 0.227/0.275/0.366 | 150 | 151 | 1 | 0 | 0 | 0 |
| R20 | 0.167/0.286/0.581 | 0.190/0.327/0.670 | 150 | 150 | 0 | 0 | 0 | 0 |
| R21 | 0.240/0.421/1.010 | 0.248/0.326/0.567 | 150 | 151 | 1 | 0 | 0 | 0 |
| R22 | 0.348/1.278/1.886 | 0.264/0.451/1.471 | 146 | 149 | 4 | 1 | 0 | 0 |
| R23 | 0.211/1.326/1.513 | 0.263/0.400/0.998 | 149 | 150 | 1 | 0 | 0 | 0 |
| R24 | 0.154/0.214/0.314 | 0.173/0.236/0.430 | 150 | 150 | 0 | 0 | 0 | 0 |
| R25 | 0.154/0.303/0.730 | 0.174/0.254/0.475 | 148 | 151 | 3 | 0 | 0 | 0 |
| R26 | 1.484/2.580/3.455 | 0.779/1.488/2.968 | 147 | 150 | 3 | 0 | 0 | 0 |
| R27 | 0.090/0.167/0.281 | 0.058/0.946/0.999 | 150 | 150 | 0 | 0 | 0 | 0 |
| get_market_slice | 0.064/0.723/1.262 | 0.027/0.072/0.096 | 151 | 151 | 0 | 0 | 0 | 0 |
| list_markets | 0.011/0.024/0.065 | 0.006/0.011/0.016 | 151 | 151 | 0 | 0 | 0 | 0 |
| plan_voyage_fuel | 0.013/0.024/0.043 | 0.010/0.028/0.051 | 151 | 151 | 0 | 0 | 0 | 0 |

- Weighted representative mix: Python 0.984 p95, Rust 0.499 p95.

## Gate (founder decisions 68 and 82, 18 Oct)

Hard gates: zero unexplained differences, no rise in 5xx/503 from Rust against Python on the same workload, and a passing integration suite. Speed blocks only at the small workload: a mix p95 regression or a per-route regression beyond 100 ms or 50 %. At the large workload, mix p95 is reported and all route regressions are fix items for the same-meaning pass. Complete valid measurements remain required at both sizes. Memory is reported against its target and does not block.

- Outcome: NO-GO.
  - zero unexplained contract differences: no-go ({'outcome': 'no-go', 'differences': 33, 'missing': [], 'invalid': []})
  - no rise in 5xx/503 from Rust against Python: pass (Rust 2 vs Python 106; offered totals/rates: {'outcome': 'pass', 'same_workload': True, 'requests': {'python': 4515, 'rust': 4515}, 'errors': {'python': 106, 'rust': 2}, 'rates': {'python': 0.023477297895902548, 'rust': 0.00044296788482834997}})
  - the integration suite passes: pass (the unchanged disposable integration suite)
  - speed (mix p95 and per-route thresholds): no-go (blocking routes: R27; fix items: R20, R24, plan_voyage_fuel)
  - memory target (Rust at most half of Python; non-blocking): pass (Rust 43.0 MiB vs Python 802.3 MiB (5.4% of Python))
- Fix items (non-blocking regressions): R20, R24, plan_voyage_fuel.
- Coverage: 0/30 routes reached the floor
  - R01: inconclusive, 700 more observations needed
  - R02: inconclusive, 698 more observations needed
  - R03: inconclusive, 698 more observations needed
  - R04: inconclusive, 700 more observations needed
  - R05: inconclusive, 698 more observations needed
  - R06: inconclusive, 700 more observations needed
  - R07: inconclusive, 698 more observations needed
  - R08: inconclusive, 698 more observations needed
  - R09: inconclusive, 700 more observations needed
  - R10: inconclusive, 698 more observations needed
  - R11: inconclusive, 700 more observations needed
  - R12: inconclusive, 698 more observations needed
  - R13: inconclusive, 698 more observations needed
  - R14: inconclusive, 698 more observations needed
  - R15: inconclusive, 700 more observations needed
  - R16: inconclusive, 700 more observations needed
  - R17: inconclusive, 700 more observations needed
  - R18: inconclusive, 700 more observations needed
  - R19: inconclusive, 698 more observations needed
  - R20: inconclusive, 700 more observations needed
  - R21: inconclusive, 698 more observations needed
  - R22: inconclusive, 700 more observations needed
  - R23: inconclusive, 700 more observations needed
  - R24: inconclusive, 700 more observations needed
  - R25: inconclusive, 698 more observations needed
  - R26: inconclusive, 700 more observations needed
  - R27: inconclusive, 700 more observations needed
  - get_market_slice: inconclusive, 698 more observations needed
  - list_markets: inconclusive, 698 more observations needed
  - plan_voyage_fuel: inconclusive, 698 more observations needed
- Integration suite: passed.

## Approved differences (exactly three)

| ID | What changed | Why approved | Proof |
| --- | --- | --- | --- |
| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; Python's four local counters can admit up to four times more | PM defines documented per-client limits as the contract | Rust exact-boundary concurrent burst/reset tests; counts excluded from parity |
| AD-02 | Rust uses one 8-slot public-read admission gate; Python uses 2 slots in each of 4 workers | Same total capacity | Mixed-concurrency test; 503 rates recorded |
| AD-03 | Rust admits `/api/news` through the same 8-slot gate | Every Rust database use holds a permit | Two-direction contention test |

- Production status: not authorized by this report.

## Large workload

- Measurement file SHA-256: 31fb9fc9211c99eea322f282918e325127bb8b7095b5fc68004320d545770cb8
- Measured source SHA: 086d4ddabffb9a4e4f4fba6d30bee15c4218d1d4

# Rust Phase 1 go/no-go

- Decision requested: continue / pause / stop.
- Candidate source SHA: 3c6ca84d41f2673e8c8d2f6b929e4e5e62c9d29a
- Python oracle SHA and corpus digest: 4ed68a2fb324835216dd3a602af5ef8e4c6e0154 / 0c0d252f5c23fcfd3cc3d051d04077d981601f1433575cf0197cfcaee7ec9060
- Schema revision and policy digest: kycd_20261015_staging / c935b83ed8f77faf74ced70849e10447ca351f9f3c1485f243b1cca36bf09693
- Rust toolchain, target, libc minimum, Cargo.lock digest, artifact digest: not measured
- Host/CPU/cgroup/PostgreSQL/PostGIS configuration: not measured
- Scope: 27 data GETs, 3 health GETs, 3 MCP transport pairs; 3 MCP tools.

## Contract and simulation

- Contract result: not measured cases, not measured passed, not measured failed, not measured skipped; not measured unapproved differences.
- Contract evidence digest: not measured
- Seeded differential simulation: size small: 10/11 groups matched, 487 pairs, 9 differences, gate no-go; size large: 10/11 groups matched, 487 pairs, 33 differences, gate no-go; size small: 1/1 groups matched, 56 pairs, 0 differences, gate pass
- Simulation evidence digest: 9e08738dc84b10da0b71dd69e8a61277762454afadbabdf87c8b8e7c069fc426
- Workload manifest digest: 85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215
- Workload sizes: {'small': {'orders': 1000, 'trades': 1000, 'news': 100}, 'large': {'orders': 100000, 'trades': 100000, 'news': 10000}}

## Adaptive probe

- Clean rate: no clean rate found.
- Stop reason: 5xx/503 at 1 RPS.
- Probe 1 RPS, 5xx/503: {'python': 2, 'rust': 0}.

## Memory

| Runtime | steady median | p95 | peak | anon median | file median | PSS median | ratio |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| python | 979.9 MiB | 1042.2 MiB | 1079.0 MiB | 962.2 MiB | 13.4 MiB | 978.4 MiB |  |
| rust | 192.1 MiB | 196.6 MiB | 204.5 MiB | 189.3 MiB | 0.0 MiB | 197.0 MiB | 0.20 |

## CPU

| Runtime | core-equivalents | usec / completed request |
| --- | ---: | ---: |
| python | 0.7824646662350986 | 808894.4071550255 |
| rust | 0.13679282617394348 | 140352.18166666667 |

## Latency (end-to-end through Caddy)

The latency series is successful (2xx) responses only. Busy (503), rate-limited (429), contract 4xx and transport failures are completed observations reported as their own counts.

| Route | Python p50/p95/p99 | Rust p50/p95/p99 | py n | rs n | py busy | rs busy | py 429 | rs 429 |
| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| R01 | 0.078/1.988/1.988 | 0.014/0.052/0.058 | 18 | 20 | 2 | 0 | 0 | 0 |
| R02 | 0.078/0.113/0.125 | 0.013/0.044/0.071 | 20 | 20 | 0 | 0 | 0 | 0 |
| R03 | 0.027/0.045/0.060 | 0.012/0.032/0.035 | 20 | 20 | 0 | 0 | 0 | 0 |
| R04 | 0.047/0.294/0.294 | 0.013/0.027/0.040 | 19 | 20 | 1 | 0 | 0 | 0 |
| R05 | 0.036/0.132/0.728 | 0.013/0.040/0.045 | 20 | 20 | 0 | 0 | 0 | 0 |
| R06 | 1.041/3.861/3.861 | 1.058/1.431/2.312 | 19 | 20 | 1 | 0 | 0 | 0 |
| R07 | 0.048/0.093/0.112 | 0.020/0.042/0.054 | 20 | 20 | 0 | 0 | 0 | 0 |
| R08 | 0.078/0.186/0.281 | 0.027/0.080/0.163 | 20 | 20 | 0 | 0 | 0 | 0 |
| R09 | 3.757/4.248/5.618 | 12.503/14.639/15.551 | 20 | 20 | 0 | 0 | 0 | 0 |
| R10 | 0.235/0.384/0.805 | 0.570/0.790/0.940 | 20 | 20 | 0 | 0 | 0 | 0 |
| R11 | 0.869/1.060/1.170 | 1.197/1.399/2.167 | 20 | 20 | 0 | 0 | 0 | 0 |
| R12 | 1.346/2.036/2.114 | 0.051/0.076/0.088 | 20 | 20 | 0 | 0 | 0 | 0 |
| R13 | 0.638/1.828/2.723 | 0.928/1.336/2.130 | 20 | 20 | 0 | 0 | 0 | 0 |
| R14 | 0.450/0.829/0.856 | 0.842/1.093/1.195 | 20 | 20 | 0 | 0 | 0 | 0 |
| R15 | 0.155/1.947/1.947 | 0.654/0.960/0.987 | 19 | 20 | 1 | 0 | 0 | 0 |
| R16 | 6.351/10.348/10.348 | 6.189/7.218/7.849 | 18 | 20 | 2 | 0 | 0 | 0 |
| R17 | 1.747/7.328/7.764 | 2.801/3.228/3.358 | 20 | 20 | 0 | 0 | 0 | 0 |
| R18 | 8.189/8.844/8.903 | 6.800/7.741/7.746 | 20 | 20 | 0 | 0 | 0 | 0 |
| R19 | 1.177/1.483/1.527 | 2.513/3.092/3.230 | 20 | 20 | 0 | 0 | 0 | 0 |
| R20 | 0.685/0.799/0.817 | 1.188/1.376/1.535 | 20 | 20 | 0 | 0 | 0 | 0 |
| R21 | 1.420/1.669/1.675 | 3.581/4.062/4.344 | 20 | 20 | 0 | 0 | 0 | 0 |
| R22 | 1.583/4.404/4.404 | 2.801/3.731/3.824 | 18 | 20 | 2 | 0 | 0 | 0 |
| R23 | 0.918/6.088/6.088 | 1.645/2.147/2.334 | 16 | 20 | 4 | 0 | 0 | 0 |
| R24 | 0.899/1.023/1.038 | 1.645/1.942/2.007 | 20 | 20 | 0 | 0 | 0 | 0 |
| R25 | 0.978/1.311/2.695 | 1.767/2.626/2.637 | 20 | 20 | 0 | 0 | 0 | 0 |
| R26 | 8.316/13.243/15.110 | 6.616/7.450/7.808 | 20 | 20 | 0 | 0 | 0 | 0 |
| R27 | 0.259/0.349/0.382 | 0.306/0.382/0.441 | 20 | 20 | 0 | 0 | 0 | 0 |
| get_market_slice | 0.084/0.276/2.773 | 0.601/0.788/0.953 | 20 | 20 | 0 | 0 | 0 | 0 |
| list_markets | 0.014/0.051/0.052 | 0.007/0.012/0.013 | 20 | 20 | 0 | 0 | 0 | 0 |
| plan_voyage_fuel | 0.019/0.035/0.037 | 0.017/0.027/0.033 | 20 | 20 | 0 | 0 | 0 | 0 |

- Weighted representative mix: Python 4.031 p95, Rust 2.771 p95.

## Gate (founder decisions 68 and 82, 18 Oct)

Hard gates: zero unexplained differences, no rise in 5xx/503 from Rust against Python on the same workload, and a passing integration suite. Speed blocks only at the small workload: a mix p95 regression or a per-route regression beyond 100 ms or 50 %. At the large workload, mix p95 is reported and all route regressions are fix items for the same-meaning pass. Complete valid measurements remain required at both sizes. Memory is reported against its target and does not block.

- Outcome: NO-GO.
  - zero unexplained contract differences: no-go ({'outcome': 'no-go', 'differences': 33, 'missing': [], 'invalid': []})
  - no rise in 5xx/503 from Rust against Python: pass (Rust 0 vs Python 13; offered totals/rates: {'outcome': 'pass', 'same_workload': True, 'requests': {'python': 600, 'rust': 600}, 'errors': {'python': 13, 'rust': 0}, 'rates': {'python': 0.021666666666666667, 'rust': 0.0}})
  - the integration suite passes: pass (the unchanged disposable integration suite)
  - speed (mix p95 and per-route thresholds): pass (blocking routes: none; fix items: R09, R10, R11, R14, R19, R20, R21, R24, R25, R27, get_market_slice)
  - memory target (Rust at most half of Python; non-blocking): pass (Rust 192.1 MiB vs Python 979.9 MiB (19.6% of Python))
- Fix items (non-blocking regressions): R09, R10, R11, R14, R19, R20, R21, R24, R25, R27, get_market_slice.
- Coverage: 0/30 routes reached the floor
  - R01: inconclusive, 960 more observations needed
  - R02: inconclusive, 960 more observations needed
  - R03: inconclusive, 960 more observations needed
  - R04: inconclusive, 960 more observations needed
  - R05: inconclusive, 960 more observations needed
  - R06: inconclusive, 960 more observations needed
  - R07: inconclusive, 960 more observations needed
  - R08: inconclusive, 960 more observations needed
  - R09: inconclusive, 960 more observations needed
  - R10: inconclusive, 960 more observations needed
  - R11: inconclusive, 960 more observations needed
  - R12: inconclusive, 960 more observations needed
  - R13: inconclusive, 960 more observations needed
  - R14: inconclusive, 960 more observations needed
  - R15: inconclusive, 960 more observations needed
  - R16: inconclusive, 960 more observations needed
  - R17: inconclusive, 960 more observations needed
  - R18: inconclusive, 960 more observations needed
  - R19: inconclusive, 960 more observations needed
  - R20: inconclusive, 960 more observations needed
  - R21: inconclusive, 960 more observations needed
  - R22: inconclusive, 960 more observations needed
  - R23: inconclusive, 960 more observations needed
  - R24: inconclusive, 960 more observations needed
  - R25: inconclusive, 960 more observations needed
  - R26: inconclusive, 960 more observations needed
  - R27: inconclusive, 960 more observations needed
  - get_market_slice: inconclusive, 960 more observations needed
  - list_markets: inconclusive, 960 more observations needed
  - plan_voyage_fuel: inconclusive, 960 more observations needed
- Integration suite: passed.

## Approved differences (exactly three)

| ID | What changed | Why approved | Proof |
| --- | --- | --- | --- |
| AD-01 | Rust enforces 60/30/10 requests per minute once per client/route; Python's four local counters can admit up to four times more | PM defines documented per-client limits as the contract | Rust exact-boundary concurrent burst/reset tests; counts excluded from parity |
| AD-02 | Rust uses one 8-slot public-read admission gate; Python uses 2 slots in each of 4 workers | Same total capacity | Mixed-concurrency test; 503 rates recorded |
| AD-03 | Rust admits `/api/news` through the same 8-slot gate | Every Rust database use holds a permit | Two-direction contention test |

- Production status: not authorized by this report.
