# Phase 0 Task 7 progress

## Scope and first test cycle

Worktree: `feat/phase0-task7`, base `cab1e0a`. No host, database or external
provider was contacted. No package was installed. No skills or subagents used.

The first requested regression command ran before implementation: **31 failed,
45 passed**. Twelve failures covered absent fence/assertions, old production
identity, missing canary origin and missing reminder units. Nineteen failures
were the known macOS Bash 3.2 deploy-test failures.

Fixed identity implementation: production uses `verdaxis_prod` with unchanged
roles; staging remains unchanged. The old pinned release still uses `verdaxis`.
The new seed guard denies both names. Production's fixed deploy path now selects
`staging`; all existing deploy locks and migration gates remain intact.

Identity/monitor regression run: **450 passed, 1 skipped** (platform-specific
monitor test). Production monitor/recovery/diagnosis files and their three test
modules came from pinned production `c08266bd6969143812dec7c5033338428cd5f159`.
No monitor unit was added to the runtime installer or activated. Reminder pairs
retain the production 15-minute calendar schedule and add dangling-state denial.
Staging reminder activation remains forbidden.

## Minimal extra files required by the identity audit

- `app/environment_database.py`: CLI environment/database boundary.
- `app/seeds/safety.py`: explicit new production-name denial.
- `deploy/monitor/backup_verify.py` and its byte manifest: exact new database
  inventory and distinct `verdaxis-prod` artifact prefix. The backup producer is
  host-owned and absent here; its approved update and restored-backup proof are
  mandatory before cutover. No producer replacement was added.
- Existing identity, CORS, backup and source-ownership tests: update fixed
  expectations and blob attestations with the owned runtime changes.

These are Task 7 identity dependencies, not work from the catalog, copy or
migration lanes. No Task 4b checkpoint/policy files were changed.

## Fence and operator preparation

The fixed external guard and read-only assertion CLI are implemented. The manual
checklist is `docs/runbooks/phase0-cutover.md`. It includes drop-in bytes, a fresh
operator-lock inode per window, exact permit fields, source/venv renames, the
old-tuple restart block and an append-only journal example. No coordinator,
repair engine or activation script was added.

The normal deploy helper now publishes `.runtime-monitor.env` after its runtime
artifact under blocked state, before candidate code or restart. The separate
publication function remains a clear extension point for Phase 1 Tasks 12b/12c.
`.gitignore` and the deploy fixture ignore this generated file. Existing gates
are unchanged. CI runs monitor tests and the fence suite in separate invocations
because the Phase 0 no-skip hook must not absorb optional monitor skips.

The external monitor's pinned backup/outbox defaults also now use `verdaxis_prod`
and `verdaxis-prod`; installed host configuration is not changed. The outbox
probe example and artifact digest were updated with it.

A PostgreSQL-runner pass exposed a fence reader false refusal when reading a file
updated its access time. Validation now compares size, modification/change times
and inode, not access time. The rerun is recorded in the final evidence.

## Resume / integration gate

Read `outputs/phase0/task-07.md` for final results and limits. Recheck the final
union revision, bootstrap policy, exact production Settings and same-revision
deploy after Task 4b lands. The one-use identity assertion intentionally refuses
an intermediate canonical revision. No checkpoint file was changed here.

Host-specific maintenance/egress, timer/cron inventory, backup producer changes,
monitor wiring, effective systemd drop-in and full rollback are Task 10 approval
gates. The Mac tests use actual subprocess locks and filesystem replacement, but
synthetic restart/readiness markers, not systemd. The late-commit test uses a
synthetic SQLite transaction; real PostgreSQL data/copy checks use the existing
runner. No claim of a completed host cutover or Linux restart is made.
