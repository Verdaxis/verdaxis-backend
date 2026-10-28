# Phase 0 Task 5 — copy and verification lane

## Scope and current gate

Work starts at `be9c893` on `feat/phase0-task5`. No host, deployed database,
provider, package install, push or other lane is used. Data is synthetic.
The provisional contract is `deploy/phase0/provisional-union.json`, digest
`849ca8f45a3bacd908be4173d28024006213c1eb2f370b8ce7d30ee8980547e6`.

**Host apply is closed.** The pinned target is `kycd_20261015_staging`, not
`p0_20261016_union`. This lane must not invent Task 4b's schema or claim final
copy acceptance. The CLI validates by default and never opens connections in
that mode. Synthetic apply requires the disposable runner's endpoint and
administrator-attested cluster identifier.

## Implementation

- `scripts/phase0/codecs.py`: explicit column mapping, bounded PostgreSQL text
  transport, EWKB/SRID, SQL NULL framing, streaming row/column hashes and safe
  positive sequence high-water calculations. JSON never passes through float.
- `scripts/phase0/copy.py`: source repeatable-read/read-only snapshot; separate
  backup/owner identities; target advisory/table locks; exact schema/baseline
  checks; reviewed cyclic FK replacement; exact named user-trigger handling;
  notice conversion; sequence checks; private attempt/failure/receipt files.
- `scripts/phase0/verify.py`: every copied table and source catalog value,
  numeric totals, target additions/empty tables, historical blockers and exact
  notice claims. Database constraints stay active except the reviewed cyclic
  edges, which are restored and validated before commit.
- A private exclusive attempt file prevents replay after every started attempt,
  even when transaction rollback leaves rows empty. Sequence changes are not
  transactional. Rebuild a failed target; there is no force/upsert mode.
- Preserve `commissions.match_id -> orders.id`. Support has no reverse
  authorization-to-order FK. Reviewed cycles are RFQ/quote plus two user
  self-reference FKs. No system trigger or replication-role bypass is used.

## Test record

The initial run failed for missing copy modules. A formatter removed fixture
imports; explicit re-exports fixed collection. The second red run had 15 missing
implementation failures and one codec test pass (the codec had been added while
that run was building its schemas). The original codec test failed before that.

The full unit command returned **2020 passed, 19 failed**, 504 warnings, 91.27 s.
All 19 failures are the known macOS Bash 3.2 `test_deploy_health.py` cases.
No other unit failure occurred.

Two disposable starts exceeded the original 60-second allowance while other
lanes ran containers. Increase only the readiness wait to 180 seconds. The
runner still removes its own container and volumes; it never removes other
lanes' resources.

The focused end-to-end copy passed (1 test, 31.26 s): 57 verified table sets,
82 source-derived rows, global next values 151 and 201. Aggregate row/column
hashes, numeric totals and schema fingerprints are retained in
`outputs/phase0/task-05-synthetic.json`. It contains no row payloads or URLs.

The existing `tests/postgres/test_runtime_role_policy.py` suite passed all
12 tests in 188.78 s. The Phase 0 runner clones a separate role-test database
and applies the existing policy there, without replication-role permission.
This does not yet prove post-copy ACL convergence on the final union target.

Final `bash scripts/run_phase0_postgres_tests.sh`: **39 passed**, no skips,
124.08 s. This includes all **23 Task 5 tests** and 16 Task 1 inventory tests.
All six failure boundaries were reached explicitly; source hashes stayed equal,
original FK/trigger definitions returned, and started/sequence-mutated/populated
targets refused retry. Python compilation and `git diff --check` also passed.

Implementation commit: `5771d1c`. No push. No live or final-union tests ran:
Task 4b has not landed and host operations are not authorized. The final host
identity path and copied-union ACL integration remain blocked, not claimed done.
No other lane-owned file was changed. The runner wait/role-test extension is
within the plan's permitted Task 1 runner ownership exception.

## Independent-review fixes — 2026-10-16

All four findings have separate implementation commits. Host apply remains closed.

- **M1 (`964c9cb`)**: pin UTC, ISO/YMD dates, postgres intervals, full float text
  precision and hex bytea on source/target and independent hash reads. The three
  formatting tests failed before the fix. They now check binary float/date
  equality after copy and detect rounded floats/swapped dates in hashes.
  An initial ISO interval setting failed catalog attestation because it changed
  rendered CHECK text; the final setting matches the frozen catalog.
- **M2 (`972bbf2`)**: refuse application-table RLS ENABLE/FORCE flags and role
  BYPASSRLS. Set transaction-local `row_security=off` so later filtered reads
  fail instead of omitting rows. No bypass or source ACL change is granted.
  Four red RLS cases now pass, including standalone verification where both
  source visibility and target rows omit the same notifications.
- **M3 (`fad8a50`)**: consume the exact expected target-only baseline set by ID
  and compare every value. Refuse missing leftovers. Missing product/delivery
  baseline tests failed before the fix; all four missing/changed tests pass.
  The provisional migration has no delivery baseline, so that regression uses
  an explicit synthetic target-only baseline, not a claimed migration row.
- **N1 (`97ac741`)**: create two temporary UNKNOWN parties in a savepoint and
  require the exact trade INSERT guard error. Always roll back the parties and
  probe trade. Both zero-trade copy and empty-target probe failed before the fix
  and now pass; source hashes and target organization counts stay unchanged.
  A disabled-trigger test also checks refusal and cleanup.

Focused runs: **4 RLS tests passed**; **9 formatting/reference/empty-history tests
passed**. Initial regression run: 10 failed, 3 passed. After correcting test
isolation and the absent delivery baseline assumption, the formatting/reference
red run returned 5 failed, 2 passed. No failure was hidden or counted as green.

Final foreground verification (both commands completed):

- `bash scripts/run_phase0_postgres_tests.sh`: **53 passed**, no skips or
  failures, 222.65 s. This includes all 14 new regressions and the existing 39
  tests. The pinned disposable Colima container was removed with `docker rm -f -v`
  by the runner. Other reviewers' containers were not touched.
- Required SQLite unit command: **2087 passed, 19 failed**, 513 warnings,
  98.21 s. Every failure is in the known macOS `tests/unit/test_deploy_health.py`
  suite; no other unit test failed.

The earlier background full PostgreSQL run ended without a completion record;
its partial output is not acceptance evidence. The completed foreground runs
replace it. Logs: `work/task5-postgres-foreground.log` and
`work/task5-unit-foreground.log`. They contain only synthetic test data.
No staging/production connection or push was performed. Host apply remains closed.

## Required Task 4b / integration re-checks

1. Freeze the final full release SHA and literal union revision, not only the
   staging base SHA. Review complete columns/types, checks, FK edges, exact
   trigger/function bytes, extension versions/membership, sequences and the
   migration-created baseline. Keep the immutable production source pin.
   Publish the final contract digest and re-run affected Rust goldens.
2. Add populated B30/B100 orders, trades, receipts and dependent histories.
   Preserve all eight product IDs, source reference values and inactive history.
   Keep UCOME distinct. Require the complete target-only baseline, not only
   membership of rows that happen to remain. No stronger check is weakened.
3. Review `CYCLIC_FKS`, import ordering, named triggers, global/owned sequences,
   all 15 added-column mappings, target-only tables and skips against that
   catalog. Keep LNG missing-evidence and lane blockers. Never derive missing
   historical terms from current orders.
4. Re-run formatting and RLS regressions on the final schema: opposing role
   DateStyle values, low extra_float_digits, UTC/microseconds, exact float bytes,
   high-precision Decimal/JSON, SQL NULL versus JSON null, EWKB and empty trade
   history. RLS ENABLE or FORCE flags and BYPASSRLS remain refused; do not grant
   a bypass. Add bytea/native-array vectors if either becomes a copied type.
   Exercise cyclic references across more than one 256-row batch and all empty,
   uncalled, sparse and cached sequence states.
5. Review host mode separately. Bind root-owned manifests and trusted bootstrap
   evidence to exact host, old/new database names, OIDs, endpoints, independently
   checked cluster identities, source revision/fingerprint, final SHA and target
   revision. Do not recapture a baseline from an arbitrary existing target.
   Require private local evidence directories and a durable attempt identity.
   Keep the synthetic-only gate until approval; no real data reaches this Mac.
6. Drain all producers and the outbox before freezing the source, including
   sequence writers. Preserve uncertain mail claims and stop senders. Enforce
   target isolation throughout import and ACL acceptance, not only at the first
   session count. Attest scratch egress, local storage and database logging:
   customer rows, private diagnostics and credentials must not leave the host.
7. Converge the approved canonical ACL bundle on the new database only. Prove
   raw app/backup authority and import-as-app denial on the copied union for
   grace, receipts/evidence, provenance, vault and control tables. Never alter
   global live roles wholesale or grant runtime import authority.
8. Repeat all six failure boundaries on the final union. Include post-commit
   receipt-publication failure and read-only recovery verification of complete
   business/reference sets, restored schema/triggers, sequence state and source
   freeze/snapshot identity. Publish aggregate counts/digests and measured runtime
   for Tasks 9/10. Recover by verification or rebuild, never replay.

## Lane ownership

Only Task 5 files and the allowed Task 1 runner are changed. `progress.md`,
application code, migration history and checkpoint policy remain untouched.
Architecture additions are recorded here for Task 7's `ARCHITECTURE.md` owner
rather than editing that shared file in parallel.
