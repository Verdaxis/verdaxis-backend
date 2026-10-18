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
- Offered Rust rates: not measured; higher than the frozen 5 RPS maximum is stricter for Rust.
- zero unexplained contract differences at both sizes: pass ({'outcome': 'pass', 'differences': 0, 'missing': [], 'invalid': []}).
- integration suite: pass (explicit suite result).
- Rust-only frozen-baseline performance evidence: inconclusive (missing Rust-only workload identity).
- Rust memory steady median: not measured; non-blocking.
- Blocking routes: none proven.
- Same-meaning fix items: none proven.

- Integration evidence: not supplied.
- Production status: not authorized by this report.
