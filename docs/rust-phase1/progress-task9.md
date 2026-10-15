# Task 9 — price discovery and trade tape

Scope: R06 and R12–R14, lane `feat/rust-phase-1-lane-9`, base `f1d53a9`.
No Python application changes, live access, push, agents or skills.

## Implementation

- Added separate REAL/DEMO arithmetic summaries, REAL-only daily VWAP, unbounded
  optional-date reference CSV, and anonymous seven-day tape.
- Every query uses `run_admitted` and the shared trade binder. There are no
  mutable catalog/owner joins in historical trade queries.
- Existing AD-01 middleware owns the 60/30/10 buckets exactly once. Routes add
  no counters. Workspace HTTP tests cover bursts, reset, client keys and headers.
- Added `market/price_quarantine.rs` (registered beneath `visibility` to avoid
  the integration-owned module registry), `sql/formal_trade_snapshots.sql` and
  `sql/price_quarantine.sql`. The metadata relation has no economic columns.
- Necessary shared edit: extracted the original formal snapshot core from
  `sql/trade_evidence.sql`. `visibility::trade_evidence_sql()` composes that core
  with the unchanged exact REAL/REAL-or-DEMO/DEMO restriction. Existing bind
  positions and public signatures do not change. No public-order SQL changed.
- The contract target uses the existing `prices` group, not nonexistent
  `reference`, `reference_csv`, or `unknown_trade_quarantine` groups. The current
  runner has no `--rust-target` flag. This follows Task 8's feature-gated test
  file pattern without editing Cargo manifests.

## Red/green evidence

- `task9-csv-red.log`: unresolved `reference_csv`, before implementation.
- `task9-red.log`: 9 cases, 31 differences against the empty Rust handlers.
- Initial implementation: 2 CSV tests passed; 9 ordinary contract cases matched.
- Expanded to 28 contract cases. One known difference remains: invalid tape
  window returns the same 500 text, but shared Rust request middleware adds
  `X-Request-ID`; Python's unhandled exception bypasses that middleware.
  Runtime HTTP composition belongs to integration, outside this lane's edit
  scope. Do not exclude this difference or call the expanded gate passed.
- Added task-local guarded REAL fixtures and shared-core comparisons. Current
  guards refuse new UNKNOWN/mixed/version-0 trades; no guard is disabled.
  Historical quarantine acceptance remains Task 4b's populated-corpus gate.
- First fixture attempt used three-place prices and was correctly refused by
  `ck_trades_numeric_values`. Corrected fixtures use valid two-place prices;
  arithmetic ties are tested separately.

## Remaining acceptance and integration work

- Task 4b must supply historical quarantine and the full populated corpus,
  recorded golden bytes and the applicable assertion-ledger acceptance proof.
- Integration must preserve Python's missing middleware headers on the tape's
  unhandled invalid-window response. Keep the failing case in `cases.json`.
- This lane does not edit `ARCHITECTURE.md` or `progress.md` under the explicit
  task-local ownership rule. The implementation map above is the integration
  owner's architecture update input.
- Do not promote this checkpoint as final Task 9 acceptance until the above
  gates pass. Verification results and resume point are appended below.

## Verification checkpoint

- Shared extraction commit: `14e581c`.
- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: 77 passed.
- `RUST_TEST_PYTHON=<absolute lane venv> bash rs/read/tests/run_postgres.sh`:
  84 passed; two market matrix tests explicitly filtered by that runner.
- Dedicated `run_market_contract.sh`: 2 passed, covering both KYC switch states
  and the original economic evidence matrix after the core extraction.
- Ordinary `prices` group: **failed**, 27/28 cases matched; invalid-window
  middleware header difference retained. The same run also passed 11 populated
  task-local HTTP probes and compared 17 formal snapshots with Python; quarantine
  was empty under the current guards. See `task9-populated2.log`.
- `venv/bin/python -m pytest tests/rust_contract -q`: 42 passed, 5 warnings.
- Required Python price/export/tape unit files: 77 passed, 11 warnings.
- `git diff --check`: passed. All started disposable runners exited and ran
  their own container/volume cleanup. No other lane's containers were removed.

## Resume point

This is an **incomplete implementation checkpoint**, not final Task 9 acceptance.
Beyond the external header integration and Task 4b gates, finish task-local query
validation: aggregate simultaneous FastAPI validation errors in declaration order,
match the full Pydantic date/datetime query grammar, and add HEAD/slash/method
contract cases. The current date parser covers the recorded date cases only.
Expand populated tests for mutable catalog edits, zero-volume historical input,
and DEMO/REAL separation assertions beyond comparator equality. Review Decimal
context limits before accepting historical numeric inputs. Do not mark the
assertion transfer complete based on the Python suite alone.

## Continuation — 16 October

The sections above are historical checkpoint evidence, not the current result.

- Authorized deviation: `runtime/http.rs` now has an internal response-extension
  marker for the outer Python ServerErrorMiddleware response. Only marked
  unhandled responses omit request-id and CORS decoration; handled 500/422/429
  responses retain their prior headers. Tests cover both paths with an Origin.
  The original 28/28 cases now match (`task9-header-green.log`).
- `prices_query.rs` validates all declared typed parameters before handler filter
  validation, collecting errors in Python declaration order. Strings retain
  last-value semantics and unknown keys remain ignored. `prices_date.rs` handles
  calendar/date-time, midnight, timezone and Unix-timestamp date forms, with
  Python error envelopes. UUID short-form errors need Python's older wording.
- Evidence assertions now check arithmetic mean (not VWAP), exact hour/week
  cutoff inclusion, beyond-end totals, short IDs and party/point privacy,
  REAL-only reference and DEMO-only tape. The 11 probes run again after product
  and delivery-point deactivation; immutable results still match.
- Numeric review: current trade guards require positive quantity <=100000 and
  price <=1000000 with bounded scale. The fixture exercises unequal quantities,
  differing scales and half-even rounding. Current guards refuse zero-volume,
  mixed/UNKNOWN and version-0 history. Do not disable guards to test those.
  Historical zero-volume/extreme numeric/quarantine acceptance waits for 4b.
- Task 13 explicitly owns method, HEAD and trailing-slash cases; skipped here
  under the continuation instruction, not an unreported coverage omission.
- A shell edit of the expanded cases did not persist. Earlier claims of 52
  cases were incorrect: those logs show 28. The retained expansion is 40 cases,
  added with the file tool; its first run exposed UUID short-form message drift.

### Final continuation checks

- Commit `a261d76`: header path and parameter validation.
- Ordinary price contract: **40/40 matched**, plus **22/22 populated probes**
  before/after catalog deactivation. Shared core: 17 rows compared; current
  quarantine count zero. `task9-40-green.log` is the final contract evidence.
- fmt and clippy passed; offline workspace **79 passed**; disposable PostgreSQL
  **86 passed**. Its two intentionally filtered market matrix tests passed in
  the dedicated runner at the earlier shared-core checkpoint.
- Python contract meta-tests **42 passed**; price/export/tape tests **77 passed**.
- Every requested check ran. No live access or push. Runner cleanup completed.
- Final acceptance still waits for Task 4b's historical corpus/golden/ledger
  gate. Task 13 owns method/HEAD/slash behavior. No threshold-count parity is
  asserted for AD-01.

Resume with Task 4b's populated price group, retaining these 40 cases and the
current guard checks. Historical numeric values outside current guard bounds
need explicit comparison before claiming that historical domain is accepted.

Temporary logs are under `work/rust-phase1/task9-*.log` and are not committed.
