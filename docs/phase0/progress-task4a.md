# Phase 0 Task 4a — checked Alembic connection

## Scope and implementation

Task 4a adds the supplied synchronous SQLAlchemy connection path in `alembic/env.py`
and the asynchronous caller in `scripts/phase0/bootstrap.py`. The caller owns one
outer transaction through attestation, migrations and exact schema verification.
The supplied path opens no engine and does not commit, roll back or close the
connection. Alembic's transaction joins the caller's transaction. An autocommit
block is refused before it can commit. Config attributes are removed in `finally`.
The ordinary Settings-driven path retains its attestation/commit/engine behavior.

The synthetic policy pins the migration file digest, literal
`kycd_20261015_staging` revision and Task 1 structural schema digest. It accepts
only the disposable runner's numeric-loopback endpoint, `phase0_bootstrap_*`
database and `phase0_owner` role. Attestation checks the URL, server endpoint,
database name/OID, session/current role, ownership and role properties on the DDL
connection. The runner's independent cluster identifier must also match that
connection's `pg_control_system()` result. No grants are added. PostgreSQL 17 in
the pinned image permits this read without an added role privilege.

The transaction-scoped bootstrap lock is acquired before the empty-relation
check. Extension-owned relations are allowed; application tables, sequences,
views, materialized views and foreign tables are not. Post-build verification
checks the exact schema and requires all noncatalog business tables to be empty.
No user, organization, grace, notice or document rows are enrolled or invented.

CLI: `venv/bin/python -m scripts.phase0.bootstrap --manifest <private-json>
--environment-file <private-env> [--apply]`. Both files must be owned mode-0600
regular files, opened without following symlinks. The env file contains only
`MIGRATOR_DATABASE_URL=...`. The default checks the manifest and policy without
opening that env file or a database connection. `--apply` is separate approval.
The manifest has exactly `mode`, `approved`, `target_revision`, and `identity`;
identity fields match the frozen `BootstrapIdentity` dataclass. Host mode is
refused. This interface is not permission to run on a live host.

## Test sequence and evidence

- Initial red run: **14 failed**, including the existing env path attempting
  `asyncio.run` instead of using the supplied connection. No implementation was
  present when this run started.
- First implementation: **14 passed**; commit `8a2155b`.
- Boundary tests exposed alias resolution and a CLI `__main__` class-identity
  mismatch. Both were corrected and retested. The raw Alembic destination must
  be literal; `head` is refused even when it resolves to the approved revision.
- Focused CLI/transaction run: **24 passed**, before adding two role-property
  cases. Final gate results are recorded below.

Synthetic evidence from that focused run (container and volumes removed):

| Field | Value |
|---|---|
| Target database | `phase0_bootstrap_b466335670984e20b6e6a6ecc8e76365` |
| Target OID / role | `24155` / `phase0_owner` |
| Client endpoint | `127.0.0.1:32801` |
| Server endpoint | `172.17.0.3/32:5432` |
| Cluster system identifier | `7696961287240179806` |
| Target engine connections during bootstrap | **1** |
| B: unrelated staging hash, before = after | `46cc100e671362e5c4f79d8d999b4e2b6d9ecc68b16eebfe652345e4c88076a4` |
| C: source hash, before = after | `93149effd08533549f30c4ab33bf2e282af291bec13c98151b7277c5f6a7eed6` |

B/C hashes cover catalog definitions and every table's rows. Successful A DDL
commits only after schema verification. Injected post-DDL verification failure
and autocommit escape both leave no `users` or `alembic_version` relation.

## Ownership and resume point

- Only deviation from the Task 4a file list: one `ARCHITECTURE.md` file-map line,
  required by project instructions. Progress is in this lane-specific file, not
  `progress.md`. No other lane's implementation files changed.
- Task 4b must replace the synthetic policy with the reviewed union revision,
  graph/schema hashes and exact host approval/source-SHA contract. Current
  hardcoded synthetic checks deliberately cannot be enabled by editing JSON.
- Task 7 owns exact `verdaxis_prod` production Settings. The production-Settings
  separate-cluster test is deferred until that identity is integrated. No
  arbitrary deployed database-name exception was introduced here.
- Task 5 may use the checked-connection pattern. No copy policy, runtime ACL,
  migration parent, ordinary checkpoint allowlist or deploy script was changed.
- No live host/database access, real data, dependency installation, push, skill
  invocation or subagent was used. `work/`, `venv` and `.claude/` are not committed.

## Final gates

- `bash scripts/run_phase0_postgres_tests.sh tests/phase0/test_bootstrap.py`:
  **26 passed**, 28 warnings, no skips.
- `bash scripts/run_phase0_postgres_tests.sh`: **65 passed**, 28 warnings,
  no skips. Containers were removed with their volumes by the runner.
- Required full unit command: **2074 passed, 19 failed**, 509 warnings.
  All failures are the known `tests/unit/test_deploy_health.py` macOS Bash 3.2
  failures (`scripts/deploy.sh: line 479: mapfile: command not found` and dependent
  assertions). There are no other failures. No deploy changes were made.
- Unchanged `tests/unit/test_migration_checkpoint.py`: **24 passed**.
- `git diff --check` and Python compile checks passed.
- Not run: production-Settings separate-cluster integration, pending Task 7's
  exact identity and Task 4b's final policy. No live tests were authorized.

Final raw logs: `work/task4a-final-focused.log`, `work/task4a-final-postgres.log`,
`work/task4a-final-unit.log`, `work/task4a-checkpoint.log`. The final full-run
fixture directory is `work/phase0-fixtures/883726f3ff3f45408bc92e4657bc3008`.
Its `bootstrap-evidence.json` retains the run-specific identity and B/C hashes.
These temporary logs and fixtures are deliberately not committed.
