# Phase 0 progress

## Phase 0 outcome — 2026-10-17

Phase 0 is deployed. Release `56d75770` has been live in production since
17 Oct 02:39 SGT (cutover window 02:19–02:39 SGT; API closed about 8 minutes)
on the new `verdaxis_prod` database at revision `p0_20261016_union`, with
frontend `5f3cd698` promoted by workflow run `38948394269`. The final accepted
Python oracle is **`56d7577085af99411a6f9c3e34e64ad929d09954`**. The window
record, deviations and open items (B11 key ring; quiet-period cleanup) are in
`outputs/phase0/acceptance.md`; the host appendix carries the post-cutover
amendments. Task-level history follows unchanged below.

## Task 1 — source fixtures and frozen mapping

Task 1 is complete. No later task was implemented. Acceptance evidence and the
resume command are in `outputs/phase0/task-01.md`.

### Later-task facts from real disposable catalogs

- Task 5: `commissions.match_id` has an actual foreign key to legacy `orders.id`
  in both pinned migration builds. The ORM/global gotcha does not describe this
  migrated schema. Preserve this dependency; do not drop it to fit ORM metadata.
- Task 5: the RFQ/quote cycle is populated and passes the production acceptance
  trigger. The pinned support schema has authorization → context/user/org and
  order → authorization links, but no reverse authorization → order FK. The
  fixture includes a consumed authorization and linked order/receipt. Inspect
  the final union catalog before choosing any additional cyclic FK changes.
- Task 4a: both historical chains build unchanged. Provision PostGIS in the
  disposable template as administrator; run Alembic as a separate LOGIN,
  NOINHERIT, non-superuser owner with no memberships. No stamp, history edit,
  supplied-connection implementation or deployed identity exception was needed.
- Tasks 2/4b: pinned source and target have 61 and 62 classified tables. There
  are 15 added shared-table columns. The target is the pinned staging base,
  not the future union revision. Update the target contract only after review.
- Tasks 4a/5: migration catalog rows contain run-time `created_at` values.
  Structural hashes exclude reference rows; per-run receipts hash every
  reference value separately. Do not ignore these values during data copying.
- Tasks 5/9: the fixtures are synthetic and include rejected/moved KYC, expired
  grace and notice audit. They confer no real admission or sending authority.
  Full import, ACL denial, sequence next-value variants and rollback proofs
  remain owned by later tasks.
- The full unit run has only the 19 known `test_deploy_health.py` failures under
  macOS Bash 3.2 (`mapfile: command not found` and dependent assertions). No
  deploy script change belongs to Task 1.

## Resume

Start Task 2, 3, 4a, 5, 6, 7 or 8 from the Task 1 commit in the assigned worktree.
Use `deploy/phase0/provisional-union.json` only as a provisional contract.
No live operation or push was performed by Task 1.
