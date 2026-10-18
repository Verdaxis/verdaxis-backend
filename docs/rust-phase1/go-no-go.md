# Rust Phase 1 go/no-go

## Founder decisions 86/87 — 18 October 2026

> Capacity update (18 Oct, founder decision 88): the eight-slot/nine-connection
> values in historical evidence do not describe the current configuration. Current
> Rust admission is configured by `read_admission_slots` (default 16), with matching
> read pool plus one readiness connection. Python is unchanged. See
> [admission-capacity.md](admission-capacity.md).

**Overall: INCONCLUSIVE; not GO.** The existing `t14d-d87` small Rust-only
measurement passes under decisions 87 and 96. Both differential contract sizes
pass. The remaining blocker is qualified large Rust-only performance evidence:
the existing large inputs lack Rust-only identity. No new measurement was run
and no deployment is authorized.

### Rule

- Python is retained as the differential correctness oracle at both sizes.
  Zero unexplained contract differences and a passing integration suite remain
  mandatory. The large differential gate now passes: the 18 October staging rerun
  compared 487 pairs under decision 85, with zero unexplained differences and no
  inconclusive sandwiches. See the contract result below; the 33 historical
  trade-tape differences are superseded, not reclassified as fresh evidence.
- Small Rust performance compares only against the committed frozen Python
  baseline, never a live Python performance cell. Weighted mix p95 must not rise.
  Per-route p95 blocks only above 100 ms or above 50% regression. Error-rate
  comparison uses exact integer count cross-products against the frozen 5xx/503
  rate, so different offered request totals are allowed.
- Large workload is Rust-only and reported under decisions 82/86. Valid baseline
  and Rust measurement evidence, including the success floor, remain mandatory.
- Rust memory is reported, not blocking. Python speed and memory are not measured.
- Missing/edited/unpinned baseline, missing routes, invalid latency, insufficient
  Rust successes, or missing/mismatched run identity give INCONCLUSIVE, never GO.

### Baseline source and attestation

Used the latest complete route-coverage small Python run at source/harness
`738fe0e85d85d7667263e8950fe4de4c05831e38`, not the older v2 run. The source file
hash was verified again during copying. Historical `workload.manifest_digest`
from that commit computed `85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215`.
The run did not itself contain a manifest digest; that fact and the PM attestation
are explicit in the baseline. Source/harness identity and 18 Oct 14:24 SGT end
time are PM-attested under decision 87, not inferred from file timestamps.

The founder freezes 106–151 successful samples per route/tool for Python only:
"below the 1,000-sample floor; frozen by founder decision 87". Rust keeps its
1,000-successful-sample floor except where decision 96 applies a lower floor
from a written rate limit: R14 requires 570 and has 596 successful responses.

R27 frozen p95 is **0.148993900 s**, 150 successes / 150 offered, zero 5xx.
The integer-microsecond relative cap permits up to 0.223491 s; 0.223492 s blocks.
The full old small Rust R27 p95 (0.946 s) is diagnostic only, not current qualified
Rust evidence. Large old threshold regressions remain same-meaning fix items;
the qualified small run has no blocking regression or same-meaning fix item.

### Rust-only measurement usage

`measure.py --rust-only` (also the default) starts only Rust, not Python.
Use equal-weight fixed windows at 5 RPS or higher; a higher rate is stricter for
Rust and is disclosed in the report. For example a 10 RPS, 3,600-second window
can offer at least 1,000 requests to every route/tool, but only successful 2xx
responses count toward the floor. Rate limits and failures are never bypassed.
A bounded run below the floor records INCONCLUSIVE; extend the reviewed duration
rather than count failed requests as successes. Rust identity records full source
and harness SHAs, manifest and baseline digests, date and offered rates. Use the
exclusive `heavy measure` helper for an actual performance run.

### Contract result — large differential rerun, 18 October 2026

**Large contract gate: PASS.** Tested source/harness
`1bae4d583c09b63d2f530b18d3afc78d1a878d8c` on the isolated staging test host
`vmi1840561.contaboserver.net`, through `verdaxis-staging-test` and `heavy run`.
Manifest digest: `85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215`.
Each group used a fresh disposable large workload (100,000 orders and 100,000
trades), both stacks, and the Python–Rust–Python sandwich from `simulate.py`.

Decision 85 (`19f84191`) was active: only exact trade-tape item bytes within a
`confirmed_at` tie may permute. Tie-key sequence/order, item sets, surrounding
bytes, total and market hours remain strict. No comparator, application or SQL
code changed. The unchanged authoritative comparator checked all saved recordings.

| Group | Pairs compared | Matches under approved rules | Unexplained differences | Inconclusive sandwiches |
| --- | ---: | ---: | ---: | ---: |
| health | 8 | 8 | 0 | 0 |
| echo | 2 | 2 | 0 | 0 |
| simple_reads | 49 | 49 | 0 | 0 |
| live_benchmarks | 4 | 4 | 0 | 0 |
| orderbook_snapshot | 7 | 7 | 0 | 0 |
| orderbook_lists | 28 | 28 | 0 | 0 |
| orderbook_map | 41 | 41 | 0 | 0 |
| prices | 56 | 56 | 0 | 0 |
| curves_legacy | 12 | 12 | 0 | 0 |
| curves_canonical | 137 | 137 | 0 | 0 |
| mcp | 143 | 143 | 0 | 0 |
| **Total** | **487** | **487** | **0** | **0** |

Difference classification: no new trade-tape raw-body ordering differences were
observed, so zero cases needed the tie exception; zero unexplained differences
remain. The former 33 differences are historical observations, not differences
from this rerun. Other existing approved comparator rules remain unchanged.

The full invocation was `simulate.py --size large`, with a 7,200-second hard
limit because it seeds 11 fresh large datasets and records three passes each.
It finished ten groups, then failed starting the final MCP container: Podman
3.4.4 timed out fetching the pinned image configuration over IPv6 from Docker
Hub. The simulation and wrapper exited **1**. This was a registry transport
failure, not a byte-comparison result; no host guard was changed. A separate
`simulate.py --size large --groups mcp` call under `heavy run 3600` completed
with simulation/wrapper exits **0/0**. All 33 saved recordings were then checked
together under `heavy run 3600`: aggregate contract gate/wrapper exits **0/0**.
No completed group was omitted, and no inconclusive group was counted as a match.
The run used staging Python 3.12.13; no version-dependent contract difference was
observed. No performance, memory or capacity measurement was run.

Local logs: `work/large-contract-d85.log`, `work/large-contract-d85-mcp.log`,
`work/large-contract-d85-summary.log`. Staging evidence:
`work/large-contract-d85/recording-01.pickle` through `recording-33.pickle` and
`summary-large.json` (includes each recording's SHA-256). The recorder wrapper
only retained results; it did not change the simulation or comparator semantics.
All lane calls finished and disposable environments were cleaned up.

This closes the large differential gate. Qualified small performance evidence
is supplied below from the existing run. Qualified large Rust-only performance
evidence remains missing; that is the sole evidence blocker to an overall GO.

### Final speed integration — decisions 87 and 96

Continued the in-progress merge of speed tip `3990151f` without aborting or
rebasing. Merge commit: `147838efa1dd61a74cb004dac93ae37dea7627c3`.
The only conflicted file was this report: retained the accepted large-contract
record and the speed lane's qualified small result. No serving query changed.

Existing run `t14d-d87` was read from the prod test lane's
`work/rust-phase1/measure-small-rust-only.json`; SHA-256
`df1662ae6b61733d527c06bccaaad6bb7df9aa161713f9e0dd2b51ddd7ea4543`.
Measured source and harness: `8a5e45369326db6a25bc94e0aa7cbbe0ff263310`.
This is the original evidence, including its original pre-decision-96 cached
INCONCLUSIVE verdict; the merged gate recomputed PASS from its unchanged samples.
The measured run used the release profile, 10 RPS for 3,600 seconds, equal-weight
schedule, 36,000 offered requests. Weighted mix p95 is **0.329429481 s** versus
**0.977916453 s** frozen Python; Rust has **19/36,000** 5xx responses
(**0.052778%**) versus **156/4,515** (**3.455150%**). Zero blocking routes;
zero same-meaning fix items. R14 has **596** successful responses against the
written 10/minute limit: `floor(0.95 × 10 × 3600 / 60) = 570` (decision 96).
Other routes/tools meet their enforced floors. No failed response was counted
as successful and no rate limit was bypassed.

Code difference from measured source to merged tip: Rust-owned configurable
admission defaults to **16 read slots**, matching read pool plus one separate
readiness connection, associated budget/harness/test updates, and gate/report
code for decision 96. `git diff 8a5e4536 147838ef -- rs/read/sql` is empty;
production Rust source changes are confined to `lib.rs`, `runtime/admission.rs`
and `runtime/config.rs`. The merged binary itself was not measured.

Contract inputs: the original small simulation at `518adc6a` plus its accepted
prices-group rerun at `086d4dda` (later group result supersedes the original
nine tie-order paths), and this lane's accepted large 487/487 summary at
`1bae4d58`. The large summary was adapted only to the report's existing input
schema; no outcomes or counts changed. Combined contract verdict: **PASS**, zero
differences, zero missing/invalid groups. Small evidence SHA-256 values:
`8dd7c4371ac32d2e58ace9f9662c1b9a2dc8bdfc816e6a418de778fedfc56b2c`
and `4e6b22baee366b665c32fb93be734ea247fe29418023bd9ce555efe3f7a844cf`.
Accepted large summary SHA-256:
`fe749add838b15901b6c9c5f5346683f05f021b98dc3071e3a754913c37e61bc`.

**Exact remaining blocker to overall GO:** the unchanged report/gate requires
valid Rust-only performance evidence at both sizes. `t14d-d87` is `size=small`.
The existing large v2 run (`e43282aee6cb5b5b5ee6fe093be88d80704e104a`, SHA-256
`27efbd9796c98dbcf29d40b0b1cc2413d687ec958e8b6717c285571512da2ab5`)
is historical paired evidence, not a qualified Rust-only run; it lacks
`mode=rust-only`, harness SHA, manifest/baseline pins and measurement date, and
its roughly 48–51 successes per route are below the floor. The gate returns
`inconclusive (missing Rust-only workload identity)` before floor admission.
Large *speed regressions* being reported-only does not waive evidence admission.
No new waiver was inferred, no identity was fabricated, and no new measurement
was run. Thus small speed and both contract gates pass, but the combined report
remains **INCONCLUSIVE**, not GO. Founder review must resolve the large-evidence
requirement or supply qualified existing evidence before an overall GO.

All requested merged-tip checks ran on staging through `heavy run 3600`:
report regeneration **0**; Rust gate targets **0**; `tests/rust_contract` **0**;
Cargo workspace tests **0**; fmt **0**; strict all-target workspace clippy **0**.
The host wrapper exited **0**. Log: `work/final-speed-integration.log`.
No application behavior or gate policy was changed to produce a GO.

### Verification

All builds/tests ran on the isolated test host through `heavy run 3600`.
The gate and whole workspace tests, Python contract suite, rustfmt check and strict
clippy passed (exit 0). Report regeneration and nonempty output check passed
(exit 0). CI for the pushed branch is checked separately. No serving SQL changed.

## Generated evidence report

# Rust Phase 1 go/no-go — decisions 86 and 87

Python is the correctness reference only. Speed and small-size error rates use the pinned frozen baseline.
Large Rust speed and errors are reported; Rust memory is reported only. Both sizes require valid Rust evidence.

## Frozen Python baseline

- Pin: `3c74f7960add211757198149241f1633ef90a165ec89a902ac40bf79c1f8ea5e`.
- Source file SHA-256: `e78ec3cb8b675b14313018f5d7522192dee1e28d0606014a33a05cbd59965d74`.
- Source and harness SHA: `738fe0e85d85d7667263e8950fe4de4c05831e38`.
- Manifest digest: `85d6617ffc7361c05446e4e485c5b00112fc88b6f76671e6c8ca552880aca215`.
- Date: 2026-10-18, ended about 14:24 SGT.
- Attestation: PM, tracker decision 87.
- Known limit: below the 1,000-sample floor; frozen by founder decision 87.
- Offered windows: [{'seconds': 15, 'rps': 1.0}, {'seconds': 900, 'rps': 5.0}].
- Weighted mix p95: 0.977916453 s.
- 5xx/503: 156/4515 (3.455150%).

| Route/tool | Frozen Python p95 (s) | Successful | Offered | 5xx | 503 |
| --- | ---: | ---: | ---: | ---: | ---: |
| R01 | 0.656618812 | 132 | 150 | 18 | 18 |
| R02 | 0.074230957 | 150 | 151 | 1 | 1 |
| R03 | 0.059955030 | 148 | 151 | 3 | 3 |
| R04 | 1.083192097 | 131 | 150 | 19 | 19 |
| R05 | 0.203624987 | 151 | 151 | 0 | 0 |
| R06 | 1.161677121 | 106 | 150 | 44 | 44 |
| R07 | 0.067208478 | 151 | 151 | 0 | 0 |
| R08 | 0.104114050 | 147 | 151 | 4 | 4 |
| R09 | 0.804938614 | 123 | 150 | 27 | 27 |
| R10 | 0.357740514 | 150 | 151 | 1 | 1 |
| R11 | 0.765786195 | 150 | 150 | 0 | 0 |
| R12 | 0.208931719 | 149 | 151 | 2 | 2 |
| R13 | 0.173041727 | 142 | 151 | 9 | 9 |
| R14 | 0.096626887 | 142 | 151 | 1 | 1 |
| R15 | 0.643691805 | 142 | 150 | 8 | 8 |
| R16 | 2.833226973 | 149 | 150 | 1 | 1 |
| R17 | 1.007588158 | 148 | 150 | 2 | 2 |
| R18 | 2.376513658 | 145 | 150 | 5 | 5 |
| R19 | 0.304355415 | 148 | 151 | 3 | 3 |
| R20 | 0.254269204 | 150 | 150 | 0 | 0 |
| R21 | 0.577582172 | 149 | 151 | 2 | 2 |
| R22 | 1.525565981 | 148 | 150 | 2 | 2 |
| R23 | 1.408000184 | 150 | 150 | 0 | 0 |
| R24 | 0.209128037 | 150 | 150 | 0 | 0 |
| R25 | 0.291546338 | 147 | 151 | 4 | 4 |
| R26 | 2.730597538 | 150 | 150 | 0 | 0 |
| R27 | 0.148993900 | 150 | 150 | 0 | 0 |
| get_market_slice | 0.616173484 | 151 | 151 | 0 | 0 |
| list_markets | 0.021636288 | 151 | 151 | 0 | 0 |
| plan_voyage_fuel | 0.039317281 | 151 | 151 | 0 | 0 |

## Small workload

- Outcome: PASS.
- Rust-only performance evidence: pass (validated).
- Offered Rust rates: [10.0]; higher than the frozen 5 RPS maximum is stricter for Rust.
- zero unexplained contract differences at both sizes: pass ({'outcome': 'pass', 'differences': 0, 'missing': [], 'invalid': []}).
- integration suite: pass (explicit suite result).
- Rust-only frozen-baseline performance evidence: pass (validated).
- Rust memory steady median: 52.9 MiB; non-blocking.
- Blocking routes: none proven.
- Same-meaning fix items: none proven.
- Measured source SHA: `8a5e45369326db6a25bc94e0aa7cbbe0ff263310`; harness SHA `8a5e45369326db6a25bc94e0aa7cbbe0ff263310`; run identity rust-only at [10.0] RPS on the equal-weight schedule, baseline pin `3c74f7960add211757198149241f1633ef90a165ec89a902ac40bf79c1f8ea5e`.
- The integration branch later adds only the 16-slot admission change and gate code; no query change follows this measurement.

Historical/unqualified Rust samples remain diagnostic when evidence admission fails.

| Route/tool | Rust p95 (s) | Successful | Required |
| --- | ---: | ---: | ---: |
| R01 | 0.037 | 1197 | 1000 |
| R02 | 0.016 | 1200 | 1000 |
| R03 | 0.016 | 1199 | 1000 |
| R04 | 0.119 | 1200 | 1000 |
| R05 | 0.045 | 1200 | 1000 |
| R06 | 0.170 | 1200 | 1000 |
| R07 | 0.023 | 1200 | 1000 |
| R08 | 0.025 | 1199 | 1000 |
| R09 | 0.361 | 1196 | 1000 |
| R10 | 0.074 | 1200 | 1000 |
| R11 | 0.179 | 1200 | 1000 |
| R12 | 0.047 | 1199 | 1000 |
| R13 | 0.110 | 1197 | 1000 |
| R14 | 0.034 | 596 | 570 |
| R15 | 0.151 | 1200 | 1000 |
| R16 | 1.040 | 1200 | 1000 |
| R17 | 0.532 | 1200 | 1000 |
| R18 | 0.915 | 1200 | 1000 |
| R19 | 0.191 | 1199 | 1000 |
| R20 | 0.147 | 1200 | 1000 |
| R21 | 0.221 | 1198 | 1000 |
| R22 | 0.399 | 1198 | 1000 |
| R23 | 0.197 | 1200 | 1000 |
| R24 | 0.144 | 1200 | 1000 |
| R25 | 0.146 | 1199 | 1000 |
| R26 | 0.925 | 1200 | 1000 |
| R27 | 0.093 | 1200 | 1000 |
| get_market_slice | 0.107 | 1200 | 1000 |
| list_markets | 0.009 | 1200 | 1000 |
| plan_voyage_fuel | 0.016 | 1200 | 1000 |

## Large workload

- Outcome: INCONCLUSIVE.
- Rust-only performance evidence: inconclusive (missing Rust-only workload identity).
- Offered Rust rates: [1.0, 5.0]; higher than the frozen 5 RPS maximum is stricter for Rust.
- zero unexplained contract differences at both sizes: pass ({'outcome': 'pass', 'differences': 0, 'missing': [], 'invalid': []}).
- integration suite: pass (explicit suite result).
- Rust-only frozen-baseline performance evidence: inconclusive (missing Rust-only workload identity).
- Rust memory steady median: 206.3 MiB; non-blocking.
- Blocking routes: none proven.
- Same-meaning fix items: none proven.

Historical/unqualified Rust samples remain diagnostic when evidence admission fails.

| Route/tool | Rust p95 (s) | Successful | Required |
| --- | ---: | ---: | ---: |
| R01 | 0.232 | 19 | 1000 |
| R02 | 0.275 | 59 | 1000 |
| R03 | 0.106 | 39 | 1000 |
| R04 | 0.206 | 15 | 1000 |
| R05 | 0.072 | 17 | 1000 |
| R06 | 4.509 | 21 | 1000 |
| R07 | 0.532 | 38 | 1000 |
| R08 | 0.419 | 19 | 1000 |
| R09 | 33.946 | 14 | 1000 |
| R10 | 2.225 | 67 | 1000 |
| R11 | 4.844 | 39 | 1000 |
| R12 | 1.100 | 26 | 1000 |
| R13 | 4.957 | 22 | 1000 |
| R14 | 4.128 | 48 | 714 |
| R15 | 5.777 | 16 | 1000 |
| R16 | 22.326 | 23 | 1000 |
| R17 | 12.470 | 19 | 1000 |
| R18 | 25.811 | 33 | 1000 |
| R19 | 11.929 | 37 | 1000 |
| R20 | 7.666 | 38 | 1000 |
| R21 | 14.371 | 19 | 1000 |
| R22 | 15.919 | 25 | 1000 |
| R23 | 10.410 | 18 | 1000 |
| R24 | 10.123 | 78 | 1000 |
| R25 | 9.350 | 38 | 1000 |
| R26 | 26.264 | 14 | 1000 |
| R27 | 29.322 | 4 | 1000 |
| get_market_slice | 1.808 | 151 | 1000 |
| list_markets | 0.053 | 151 | 1000 |
| plan_voyage_fuel | 0.129 | 151 | 1000 |

- Integration evidence: Accepted integration at 70908804; current merged-tip checks recorded separately.
- Production status: not authorized by this report.
