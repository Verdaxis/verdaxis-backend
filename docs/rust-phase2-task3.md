# Phase 2 Task 3 — scoped evidence verdicts

## Source and merge

Merge `810881f7` integrates accepted gate lane `92a97587` without rebasing.
The architecture conflict retained both sides. Host merge tests: Python 166
passed (exit 0), Rust gate/frozen gate 31 passed (exit 0), auth 2 passed (exit 0),
format exit 0. Push exit 0. First immediate CI query returned 1 because checks
were not registered yet. Final #88 watch exit 0: test, postgres-phase0,
postgres-analytics, offline and contract passed; publication skipped.

## Interface

`python -m scripts.rust_contract.phase2_report` requires the declared manifest,
scope, evidence directory, candidate SHA, binary SHA-256 and trusted provenance
public-key path. Missing scope or identity is INCONCLUSIVE. The evidence directory
contains `index.json` mapping each required evidence kind to a relative path and
SHA-256. Files outside that directory and changed hashes refuse.

Each run binds its full identity, interval, exact required cases, child exit,
skips, failures and runtime/size/KYC cells. Response kinds carry raw HTTP pairs;
the gate recomputes comparison with the pinned Phase 2 profile. State/effect kinds
carry before/after state and effects, not a free-standing pass boolean. Integration
requires nonzero testcase elements, no skips and no failures. Artifact qualification
uses the existing signed-provenance verifier with a caller-selected trusted key,
not an evidence-supplied `signature_verified` boolean.

The committed inventory/manifest determine required coverage. A caller cannot
remove a route, scenario, runtime, size or policy state. Booleans are not counts.
Production scope adds measurement, benchmark, rehearsal and staging coverage;
a disposable-artifact GO is not a production-cutover GO. Valid failures take
precedence over missing evidence and retain the incomplete list.

JSON is written first. Markdown is rendered from that JSON only. Exit 0 = GO,
1 = NO_GO, 2 = INCONCLUSIVE. No live data or actual candidate GO is claimed.

Shared code is reused, not copied: report digest/write and route regression
thresholds, measurement percentile/weighted mix, comparison authority, and signed
artifact provenance verification. No Phase 1 policy or frozen baseline was changed.
Phase 2 measurement retains the approved Phase 2 manifest contract; the Phase 1
frozen public-read baseline is not treated as a CRUD baseline. Pooled p95 and
weighted route p95 are reported separately; memory size is not a blocker.

## Gates

All tests ran through the host heavy helper with a 3600-second execution limit.

| Gate | Exit | Result |
| --- | ---: | --- |
| Task 3 RED | 1 | 34 missing-module failures |
| First GREEN | 1 | 33 passed; failure fixture had inconsistent failure total |
| Corrected raw-proof GREEN | 0 | 34 passed |
| Final gate + frozen-baseline suite | 0 | 60 passed |
| Rust format | 0 | Passed |
| Rust workspace/all-target Clippy | 0 | Passed |

Logs: `work/rust-phase2/task3-{red,green,green2,final}.log`.

## Resume

Task 4 follows. Task 6 remains blocked on PM handoff of final Phase 1 runtime.
Later producers must emit these proof fields, not success summaries. Run final
candidate and cutover gates only after their required real evidence exists.
