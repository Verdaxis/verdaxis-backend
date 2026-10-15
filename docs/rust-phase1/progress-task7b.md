# Task 7b — Blocked on legacy list ordering

Date: 2026-10-16. Base: `77842676`, branch `feat/rust-phase-1-lane-7b`.
Scope: R16–R18 and R26 only. No implementation acceptance is claimed.

## Required stop: unordered ties

The task brief requires a stop and PM decision if Python leaves a list tie
unordered. Source inspection confirms this condition inside Task 7b:

- R18 `/api/orderbook/with-ci` orders only by `created_at DESC`
  (`app/routers/orderbook.py:1066-1070`).
- R26 `/api/orderbook` orders only by `created_at DESC`, then applies the
  effective offset and limit (`app/routers/orderbook.py:1790-1794`).
- Corpus construction sets every order's `created_at` and `updated_at` to the
  fixed clock (`scripts/rust_contract/seed_corpus.py:261-265`).
- R16/R17 do have complete tie-breakers: price sorts use acceptance ordinal
  and ID; quantity uses created time and ID; newest uses created time and ID
  (`app/routers/orderbook.py:246-266`). These routes are not the blocker.

A local, read-only Python inspection of the accepted golden response bodies at
oracle `a127672ee4c82fafbb4c633eb84f66bbdc676e75` confirmed:

| Case | KYC enabled rows | KYC disabled rows | Created time of every row |
| --- | ---: | ---: | --- |
| R18-with-ci | 17 | 21 | 2026-10-15T12:00:00Z |
| R26-orderbook | 17 | 21 | 2026-10-15T12:00:00Z |
| R26b-orderbook-page | 2 | 2 | 2026-10-15T12:00:00Z |

These are six inspected responses, not six executed contract tests. PostgreSQL
has no guaranteed order among these equal keys. A different query plan can
change both array order and which tied rows fall on a page. A comparison rule
that only ignores order within a returned page does not resolve page membership.
Decisions 18 and 22 apply only to news and price summaries, not these routes.

## Decision needed

The PM must define R18/R26 tie ordering and pagination acceptance before this
lane proceeds. Do not add a Rust-only ID tie-breaker, change Python, remove ties
from the corpus, or widen comparator exceptions without that decision.
An approved deterministic Python tie-breaker would require coordinated oracle
and golden updates; an alternative comparison rule must address page membership,
not only permutations within the returned array.

## Checks and changes

- Source and golden inspection found the required stop condition before Step 1.
- No failing test, route implementation, SQL, fixture, golden or comparator edit.
- No new shared files. Only this task progress file was added.
- No Cargo, PostgreSQL, disposable HTTP, Python contract or Python unit suites
  ran: implementation stopped at the explicit PM gate. No passing gate is claimed.
- No Docker container, live host, live database, skill or subagent was used.
- No push. Documentation-only commit; `git diff --check` checked before commit.

## Resume point

Read the PM's ordering decision and any resulting oracle update first. Then
finish the Task 7a code review and start Task 7b Step 1 (failing test first).
Preserve Task 7a's separate public response projection, shared visibility rules,
one captured instant, Singapore date, batched benchmarks and explicit recorded
database time zone. Run all requested gates after implementation. This stop
neither completes Task 7b nor authorizes work on another task.

## PM decision 23 — 2026-10-16 (supersedes the stop)

The PM classified unordered legacy pagination as a user-facing Python bug.
R18 and R26 now use `created_at DESC, id DESC` in both languages. No other
Task 7b route needed a Python ordering repair: all four R16/R17 sorts already
have complete tie-breakers. The Python freeze permits this bug fix.

Python TDD: the two parameterized tied-timestamp tests failed before the fix
and passed after it. The initial edit hook reformatted unrelated code; commit
`b7ba7f81` removes that formatting. Net application changes are the two ORDER BY
clauses only. Commits: `5653b128`, `b7ba7f81`.

## Oracle refresh

The recorder ran all ten current non-MCP groups twice in each KYC state.
Final oracle: `a417cf5136d3af755360ee0b8e87c7de0695bd29`; 117 cases per state,
234 total; `TOTAL_UNSTABLE=0; RECORD_FAILURES=0` in both states. MCP remains
excluded under decision 19, not silently omitted. Policy, MCP catalog and manifest
were regenerated. Catalog and nine terms-vector files were regenerated with
unchanged bytes. Public schema-order data was regenerated and checked against
Python models, then bound to the final oracle SHA. Corpus publication: `5b293e2c`.

No golden body was hand-edited. The prior corpus is retained only in ignored
`work/` for diagnosis. The new recording includes 70 cases already added by
merged lanes since the prior 164-case corpus. Among existing cases, response
body changes are readiness SHA and R18/R26 order/page membership. Four price
records add decision 22 comparison metadata (`price_summary_tokens`); their
response bytes and all pre-existing fields are unchanged.

The old list group included three Task 7c cases. Its first implemented run
matched all Task 7b cases but failed those three unimplemented routes (11 paths).
Commit `a417cf51` moves R19, R27 and the negative compatibility-listing probe to
`orderbook_map`; all are retained in both-state recordings. No comparator rule
was changed. Integration must mount the compatibility route for that group;
this lane does not implement Task 7c or edit harness dispatch.

## Implementation and regression coverage

- Four routes use `run_admitted`, `OrderQuery`, `OrderSurface::Orderbook`, bound
  filters/pagination and one captured instant. The benchmark helper uses that
  instant's Singapore date and one request-local cache batch.
- BID retains its separate total query. ASK uses the window count and a fallback
  count only when a nonzero offset returns no row. No snapshot depth cap applies.
- Exact opposing slice/provenance keys select public opposite-side prices in one
  query. The opposing scope keeps Python's default off-spec exclusion even when
  the requested page includes off-spec orders. Legacy lists keep crossed=false.
- The existing snapshot DTO was moved unchanged into the new shared
  `market/order_projection.rs`; snapshot and lists consume it. The type is
  crate-private. Public schema validation remains separate from execution
  validation, with the digest computed over private input before redaction.
- This extraction requires small edits to snapshot imports and market module
  registration. No existing shared SQL, predicate or catalog policy was changed.
  No new shared relation was needed; `orderbook/opposing.sql` is only a consumer
  aggregation over `eligible_orders`.
- Pure tests cover alias precedence and checked offset arithmetic. Disposable
  comparisons cover all four sort modes, page boundaries, totals beyond the end,
  aliases, region/point names, empty fuel filters, malformed queries, full DTOs,
  digests and benchmark/crossed values. Source regressions caught enum error order
  and Python's empty-fuel-as-no-filter behavior; both were repaired.
- The CI probe reuses disposable corpus rows with positive energy density to
  exercise non-null overlays. LNG overlays stay null; financial benefits remain
  UNPRICED with zero adjustment. No fixture IDs were added.
- Disposable database statement logging verifies one benchmark SELECT for a full
  ASK page. The setting is restored in a finally block before container removal.
- The assertion ledger adds the new Python pagination regression without changing
  other tasks' acceptance state.

## Final verification

Checks on implementation `6e9652db` (or identical source immediately before that
commit), with full HEAD exported before Cargo:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed; all-features
  clippy also passed.
- `cargo test --workspace`: **110 passed**.
- `RUST_TEST_PYTHON=$PWD/venv/bin/python bash rs/read/tests/run_postgres.sh`:
  **118 passed**; the runner's two dedicated matrix tests remain explicitly
  filtered for their separate runner.
- `orderbook_lists --seed-mode corpus`: **12/12 cases** in each KYC state,
  plus **142 enabled / 154 disabled** exact edge comparisons, a non-null CI
  overlay comparison per state, and one-benchmark-statement proof per state.
- Regression groups in both KYC states: **simple_reads 29/29**, **prices 40/40**
  plus both 17-probe passes, **curves_legacy 4/4** plus existing matrix/scale
  probes, **orderbook_snapshot 3/3** plus all snapshot edge/isolation probes.
- `venv/bin/python -m pytest tests/rust_contract -q`: **95 passed**.
- Task Python suites (marketplace filters, crossing filters, schemas, CI fields,
  router and live benchmarks): **83 passed**.
- Full Python unit suite: **2027 passed, 19 failed**. All 19 are the known
  `tests/unit/test_deploy_health.py` macOS Bash 3.2 failures (`mapfile: command
  not found`). This is not a green full-unit gate; no deploy code was changed.
- Assertion ledger and policy/terms export checks passed. `git diff --check`
  passed. No requested task gate was skipped. MCP recording stays deferred by
  decision 19; no MCP acceptance is claimed.

One initial snapshot regression attempt passed the corpus and early edge probes
but its second server refused startup (`connect failed`). That run overlapped
Cargo builds across changing HEAD identities. The unchanged gate passed when
rerun at a stable identity; the failure log is retained, not hidden.

Logs are under `work/rust-phase1/task7b-*`.
No live host, live database, push, skill or subagent was used.


## Integration handoff

Task 7b implementation is ready for independent review. Keep the oracle SHA
`a417cf5136d3af755360ee0b8e87c7de0695bd29` when merging its generated artifacts;
do not relabel them with the implementation commit. The PM adds decision 23
to the plan. Task 7c owns the moved compatibility cases and their dispatch.
Architecture integration should record `market/order_projection.rs` as the
shared snapshot/list DTO and the four new public list routes; this lane leaves
`ARCHITECTURE.md` integration-owned under the task file allowlist.
