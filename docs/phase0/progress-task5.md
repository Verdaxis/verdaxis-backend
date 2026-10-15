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

Final full Phase 0 counts will be recorded after the verification run.

## Required Task 4b / integration re-checks

1. Freeze the actual union catalog, complete columns/types, checks, FK edges,
   trigger/function bytes, extensions, owned/global sequences and baseline.
   Update only reviewed copy-manifest differences. Re-run all copy proofs.
2. Add populated B30/B100 history to the union target proof. Preserve all eight
   product IDs and source catalog values. The pinned staging checks cannot
   accept all production biofuel snapshots; no checks are weakened here.
3. Bind final host manifests to root ownership, exact old/new database names,
   OIDs, endpoint/cluster identities, host and full SHAs, source fingerprint,
   target revision and exact bootstrap baseline. Replace the synthetic-only
   gate only after separate host-mode review; no real data may reach this Mac.
4. Integrate the approved canonical ACL convergence bundle on the new database
   only; re-run raw app/backup denial and import-as-app tests on the copied union
   target. Runtime import authority must remain absent.
5. Verify the source outbox is drained after all producers stop. Preserve
   uncertain mail claims; no notice/reminder sender runs during copying.
6. Re-run numeric/JSON/null/EWKB, cyclic FK, trigger restoration, sequence
   next-value and all six failure-boundary tests on the final schema. Review
   any newly discovered owned sequence or FK; do not silently broaden policy.
7. Publish final counts, schema/data digests and measured runtime for Tasks 9/10.
   Receipt-publication failure after commit requires verification, never replay.

## Lane ownership

Only Task 5 files and the allowed Task 1 runner are changed. `progress.md`,
application code, migration history and checkpoint policy remain untouched.
Architecture additions are recorded here for Task 7's `ARCHITECTURE.md` owner
rather than editing that shared file in parallel.
