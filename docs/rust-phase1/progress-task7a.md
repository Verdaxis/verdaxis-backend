# Task 7a — Coherent public order-book snapshot

Base: `c4e4b08`, branch `feat/rust-phase-1-lane-7a`. Scope: R15 only.
Implementation/test commit: `c969dea`. The following documentation-only commit
records acceptance and remaining integration notes.
No push, live host, live database, Python feature change, agent or skill.

## Implementation

- `routes/orderbook_snapshot.rs` uses `run_admitted`, a read-only REPEATABLE READ
  transaction, `OrderSurface::Orderbook`, the shared bind helper, and one captured
  instant. The shared benchmark helper derives its Singapore date from that instant.
- `sql/orderbook/snapshot.sql` ranks eligible rows by side-specific price,
  acceptance ordinal and ID, then selects at most 15 per side. Organization tier
  is a projection join, not a replacement visibility predicate.
- One request-local `LiveBenchmarkCache` loads all complete normalized keys in
  one batch. The existing REAL/DEMO benchmark blend remains unchanged. Crossed
  flags use only returned depth and keep provenance separate within the exact slice.
- Public fields are allowlisted. The digest consumes private terms before
  redaction. Numeric text retains stored scale, and typed nested objects use
  Python response-schema field order rather than JSONB order.
- New shared data: `rs/read/src/market/orderbook_query.json`. It records the
  oracle enum declaration order and public typed-schema field order at `a127672`.
  The task-local Python comparison checks every field list against Python and
  the SHA against the policy artifact. No executable product names or shared
  eligibility SQL changes. This is serialization data, not a new market policy.

## Tests and evidence

- Red: `task7a-red-unit.log` has the expected 404 versus 422 failure;
  `task7a-red-corpus.log` has nine differences across the three populated cases.
- Pure task tests cover required-field ordering, window normalization, repeated
  scalar handling, invalid-window detail, and exclusion of private/other-task routes.
- The task-local contract target compares the unchanged three-case corpus first,
  then invokes `orderbook_snapshot_oracle.py`. It reuses corpus source rows and
  creates only `snapshot7a:` UUIDs in isolated 2032 windows.
- Edge comparisons cover 16 equal-price orders per side (15 returned in acceptance
  order), mixed provenance, schema-valid but execution-ineligible future quality
  evidence, privacy, every catalog lane at SPOT, empty slices and query errors.
- Disposable PostgreSQL statement logging proves exactly one ranked-depth SELECT
  and one benchmark SELECT on a nonempty request. No persistent logging change.
- A separate feature-gated test server waits on an advisory-lock barrier after
  depth. A writer commits a price change before benchmark selection. The blocked
  request returns old depth/old benchmark; the next returns new/new.
- A second clock-only server advances one microsecond to expiry equality. The row
  disappears without an order write. All test servers use the recorded database
  time zone and are terminated in `finally` blocks.
- The test barrier is compiled only with `contract-tests`, requires environment
  `test`, and is configured by the test process, never by a request parameter.

## Verification

Logs are uncommitted under `work/rust-phase1/task7a-*.log`.
Every Cargo invocation exports the current full HEAD as `RELEASE_SHA`.

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: **105 passed**, including digest vectors.
- `RUST_TEST_PYTHON=$PWD/venv/bin/python bash rs/read/tests/run_postgres.sh`:
  **113 passed**; the runner explicitly filters its two dedicated market matrices.
- Populated snapshot group: **3/3 cases matched** in each KYC state, plus task-local
  edge comparisons, statement count, concurrent writer and expiry boundary.
- Python contract suite and the named `test_orderbook_live_benchmarks.py`:
  **89 passed** (80 contract and 9 benchmark/snapshot tests). The separate required
  `venv/bin/python -m pytest tests/rust_contract -q` also passed: **80 passed**.
- Repeated fmt, clippy and workspace tests on committed `c969dea`: passed,
  **105 workspace tests**.
- Full Python unit suite: **2025 passed, 19 failed**. All failures are the existing
  `test_deploy_health.py` macOS Bash 3.2 limitation: `mapfile: command not found`.
  No deploy code was changed. This is not a green full-unit gate.

## Deviations and integration notes

- The current runner has no `--rust-target` flag. Its group-to-target mapping runs
  the new `rs/contract/tests/orderbook_snapshot.rs` automatically; no manifest or
  harness dispatch edit was needed. HTTP tests remain behind `contract-http`.
- Added a task-local Python edge probe and contract target to exercise the actual
  handler, rather than claiming SQL composition alone proves coherence.
- Added shared schema-order data because catalog display order does not encode
  Python enum declaration order. No existing shared SQL was edited.
- Golden data, corpus construction and `progress.md` were not changed. The
  snapshot has an explicit acceptance-ordinal/ID tie-breaker; list-route full ties
  remain Task 7b's issue.
- The corpus cancels its deliberately malformed private certificate-holder row.
  Reopening a copy exposed a separate malformed-history contract: Python's public
  response schema can ignore that private field, while shared Rust full-term
  redaction rejects it. This is outside the accepted positive corpus; retain it as
  a shared projection follow-up. The benchmark-exclusion probe instead uses valid
  stored schema with a future sample date, so it isolates typed qualification.
- Keep ARCHITECTURE.md integration-owned under this lane's file allowlist. Its
  eventual update should describe the admitted snapshot transaction and the
  schema-order data dependency above.

## Independent review repair — 2026-10-16

M1 is repaired. The snapshot now calls `public_response_terms`, which validates
public FAME/LNG schemas independently of full execution declarations. FAME
private identity and nested traceability fields are ignored; public required
fields and quality-result bounds remain checked. LNG removes its two private
identity fields before declaration validation. Existing validated-declaration
helpers retain their strict boundary. Digest input and benchmark qualification
remain unchanged.

TDD: the new unit probe failed on blank `certificate_holder`; the disposable
HTTP comparison failed on `negative-fame-holder`. The HTTP regression reopens
only a copy of Task 4b's labelled malformed fixture in a separate window. It
compares full response bytes, verifies both rows remain public, and checks that
only the qualified row sets the benchmark. A matching LNG case checks both
private identity fields. Pure tests also cover malformed nested private evidence
and missing public fields. No golden data or Python feature code changed.

This resolves the malformed-history deferral above. Architecture documentation
now records the public-response boundary. Logs use `work/rust-phase1/task7a-fix-*`.

## Resume point

R15 implementation and its local gates are recorded here. The PM can merge the
lane after review. Task 7b, routing assembly, shared malformed-history projection,
Phase 0 work and any deployment remain separate work. No live action is authorized.
