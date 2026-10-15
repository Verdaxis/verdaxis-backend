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

## Resume

Finish the fence composition, assertion and one-use checklist proofs. Record
final checks in `outputs/phase0/task-07.md`. Recheck the final union revision,
bootstrap policy and same-revision deploy after Task 4b lands. No host operation
is authorized by this source work.
